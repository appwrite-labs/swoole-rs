//! Upstream's `core-tests/src/network/socket.cpp`, one function per TEST with the same
//! inputs and expected values, over the hybrid's `network::Socket`: a Rust record
//! (`swr_v1_socket`, native/reactor/swoole_rs_socket.h) whose out-of-line members are
//! Rust exports under their upstream symbols and whose inline members call
//! `swr_v1_socket_*`. [`Socket`] makes the same calls the header's inline members make.

use std::ffi::{CStr, c_char, c_int, c_ulong, c_void};
use std::mem::{MaybeUninit, offset_of};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::address::Address;
use super::client::Client;
use super::socket::{
    EADDRNOTAVAIL, ECONNREFUSED, SHUT_RD, SHUT_RDWR, SHUT_WR, SW_SOCK_TCP, SW_SOCK_TCP6,
    SW_SOCK_UDP, SW_SOCK_UDP6, SW_SOCK_UNIX_DGRAM, TEST_HOST, TEST_HTTP_EXPECT, TEST_PORT,
    contains, get_random_port, http_get_request,
};
use super::{
    AF_INET, AF_INET6, EBADF, EINVAL, ENOENT, ETIMEDOUT, SOCK_DGRAM, SOCK_STREAM, SW_ERR,
    TEST_HTTP_DOMAIN, TEST_STR, c_str, errno, last_error,
};

const TEST_DATA: &[u8] = b"hello swoole, hello world, php is best\0";
const TEST_DOMAIN_BAIDU: &str = "www.baidu.com";
const SW_OK: c_int = 0;
const SW_WAIT: c_int = 2;
const SW_REDUCE_SIZE: c_int = 7;
const SW_SOCK_UNIX_STREAM: c_int = 5;
const SW_SOCK_RAW: c_int = 7;
const SW_SOCK_RAW6: c_int = 8;
const SW_FD_DGRAM_SERVER: c_int = 2;
const SW_FD_STREAM: c_int = 4;
const SW_FD_STREAM_CLIENT: c_int = 17;
const SW_TIMEOUT_CONNECT: c_int = 2;
const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_ONCE: c_int = 1 << 12;
const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
const SW_BACKLOG: c_int = 512;
const SW_SOCKET_RETRY_COUNT: u32 = 10;
const SW_STACK_BUFFER_SIZE: usize = 65536;
const SW_ERROR_INVALID_PARAMS: c_int = 505;
const SW_ERROR_FILE_EMPTY: c_int = 702;
const SW_ERROR_SSL_RESET: c_int = 1017;
// glibc's values (the core-tests build is Linux).
const AF_LOCAL: c_int = 1;
const SOCK_RAW: c_int = 3;
const SOCK_RDM: c_int = 4;
const EPROTONOSUPPORT: c_int = 93;
const ESOCKTNOSUPPORT: c_int = 94;
const ENOBUFS: c_int = 105;
const EMSGSIZE: c_int = 90;
const EAGAIN: c_int = 11;
const MSG_DONTWAIT: c_int = 0x40;
const MSG_WAITALL: c_int = 0x100;
const SIGPIPE: c_int = 13;
const SIG_IGN: usize = 1;
// OpenSSL's codes (the SSL_F_* and BIO_F_* function codes are 0 in OpenSSL 3).
const ERR_LIB_SSL: c_int = 20;
const ERR_LIB_BIO: c_int = 32;
const ERR_LIB_USER: c_int = 128;
const SSL_R_LIBRARY_HAS_NO_CIPHERS: c_int = 161;
const SSL_R_PROTOCOL_IS_SHUTDOWN: c_int = 207;
const SSL_R_SSL_HANDSHAKE_FAILURE: c_int = 229;
const SSL_R_SSLV3_ALERT_BAD_CERTIFICATE: c_int = 1042;
const SSL_R_SSLV3_ALERT_CERTIFICATE_EXPIRED: c_int = 1045;
const BIO_R_BROKEN_PIPE: c_int = 124;

/// `swr_v1_socket`: `network::Socket`'s fields, Rust's `SocketRecord`.
#[repr(C)]
struct Record {
    fd: c_int,
    fd_type: c_int,
    socket_type: c_int,
    events: c_int,
    enable_tcp_nodelay: bool,
    kernel_nobufs: bool,
    flags: [u8; 18],
    chunk_size: u32,
    out_buffer: *mut c_void,
    send_timer: *mut c_void,
    buffer_size: u32,
    output_flags: [u8; 4],
    object: *mut c_void,
    ssl: *mut c_void,
    tls: [u32; 2],
    timeouts: [f64; 4],
    last_times: [f64; 2],
    buffers: [*mut c_void; 3],
    totals: [usize; 2],
    info: Address,
}

const _: () = assert!(
    offset_of!(Record, out_buffer) == 40
        && offset_of!(Record, timeouts) == 88
        && offset_of!(Record, info) == 176
        && size_of::<Record>() == 296,
    "swr_v1_socket's layout moved"
);

unsafe extern "C" {
    fn free(pointer: *mut c_void);
}

/// `swoole::Event`.
#[repr(C)]
struct Event {
    fd: c_int,
    reactor_id: i16,
    fd_type: c_int,
    socket: *mut Record,
}

type Handler = unsafe extern "C" fn(*mut c_void, *mut Event) -> c_int;

unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole11make_socketE12swSocketType8swFdTypei"]
    fn make_socket3(kind: c_int, fd_type: c_int, flags: c_int) -> *mut Record;
    #[link_name = "_ZN6swoole11make_socketE12swSocketType8swFdTypeiiii"]
    fn make_socket6(
        kind: c_int,
        fd_type: c_int,
        domain: c_int,
        sock_type: c_int,
        protocol: c_int,
        flags: c_int,
    ) -> *mut Record;
    #[link_name = "_ZN6swoole18make_server_socketE12swSocketTypePKcii"]
    fn make_server_socket4(
        kind: c_int,
        address: *const c_char,
        port: c_int,
        backlog: c_int,
    ) -> *mut Record;
    #[link_name = "_ZN6swoole7network6Socket4freeEv"]
    fn socket_free(socket: *mut Record);
    #[link_name = "_ZN6swoole7network6Socket12connect_syncERKNS0_7AddressE"]
    fn socket_connect_sync(socket: *mut Record, address: *const Address) -> c_int;
    #[link_name = "_ZN6swoole7network6Socket13set_fd_optionEii"]
    fn socket_set_fd_option(socket: *mut Record, nonblock: c_int, cloexec: c_int) -> bool;
    #[link_name = "_ZN6swoole7network6Socket23set_kernel_read_timeoutEd"]
    fn socket_set_kernel_read_timeout(socket: *mut Record, timeout: f64) -> bool;
    #[link_name = "_ZN6swoole7network6Socket24set_kernel_write_timeoutEd"]
    fn socket_set_kernel_write_timeout(socket: *mut Record, timeout: f64) -> bool;
    #[link_name = "_ZN6swoole7network6Socket4corkEv"]
    fn socket_cork(socket: *mut Record) -> bool;
    #[link_name = "_ZN6swoole7network6Socket6uncorkEv"]
    fn socket_uncork(socket: *mut Record) -> bool;
    #[link_name = "_ZN6swoole7network6Socket4recvEPvmi"]
    fn socket_recv(socket: *mut Record, buf: *mut c_void, n: usize, flags: c_int) -> isize;
    #[link_name = "_ZN6swoole7network6Socket4sendEPKvmi"]
    fn socket_send(socket: *mut Record, buf: *const c_void, n: usize, flags: c_int) -> isize;
    #[link_name = "_ZNK6swoole7network6Socket4peekEPvmi"]
    fn socket_peek(socket: *const Record, buf: *mut c_void, n: usize, flags: c_int) -> isize;
    #[link_name = "_ZN6swoole7network6Socket13recvfrom_syncEPcmiPNS0_7AddressE"]
    fn socket_recvfrom_sync(
        socket: *mut Record,
        buf: *mut c_char,
        n: usize,
        flags: c_int,
        address: *mut Address,
    ) -> isize;
    #[link_name = "_ZN6swoole7network6Socket10send_asyncEPKvm"]
    fn socket_send_async(socket: *mut Record, data: *const c_void, n: usize) -> isize;
    #[link_name = "_ZN6swoole7network6Socket9recv_syncEPvmi"]
    fn socket_recv_sync(socket: *mut Record, data: *mut c_void, n: usize, flags: c_int) -> isize;
    #[link_name = "_ZN6swoole7network6Socket13sendfile_syncEPKclm"]
    fn socket_sendfile_sync(
        socket: *mut Record,
        file: *const c_char,
        offset: i64,
        length: usize,
    ) -> c_int;
    #[link_name = "_ZN6swoole7network6Socket11sendto_syncERKNS0_7AddressEPKvmi"]
    fn socket_sendto_sync(
        socket: *mut Record,
        address: *const Address,
        buf: *const c_void,
        n: usize,
        flags: c_int,
    ) -> isize;
    #[link_name = "_ZNK6swoole7network6Socket5cleanEv"]
    fn socket_clean(socket: *const Record);
    #[link_name = "_ZN6swoole7network6Socket14check_livenessEv"]
    fn socket_check_liveness(socket: *mut Record) -> bool;
    #[link_name = "_ZN6swoole7network6Socket9read_syncEPvm"]
    fn socket_read_sync(socket: *mut Record, buf: *mut c_void, n: usize) -> isize;
    #[link_name = "_ZN6swoole7network6Socket10write_syncEPKvm"]
    fn socket_write_sync(socket: *mut Record, buf: *const c_void, n: usize) -> isize;
    #[link_name = "_ZN6swoole7network6Socket8ssl_sendEPKvm"]
    fn socket_ssl_send(socket: *mut Record, buf: *const c_void, n: usize) -> isize;
    #[link_name = "_ZN6swoole7network6Socket8ssl_recvEPvm"]
    fn socket_ssl_recv(socket: *mut Record, buf: *mut c_void, n: usize) -> isize;
    #[link_name = "_ZN6swoole7network6Socket20ssl_get_error_reasonEPi"]
    fn socket_ssl_get_error_reason(reason: *mut c_int) -> *const c_char;
    #[link_name = "_ZNK6swoole6Buffer6lengthEv"]
    fn buffer_length(buffer: *const c_void) -> u32;
    #[link_name = "_Z15swoole_ssl_initv"]
    fn swoole_ssl_init();
    #[link_name = "_Z20swoole_random_stringPcm"]
    fn swoole_random_string(buf: *mut c_char, size: usize);
    #[link_name = "_Z16swoole_event_addPN6swoole7network6SocketEi"]
    fn swoole_event_add(socket: *mut Record, events: c_int) -> c_int;
    #[link_name = "_Z24swoole_event_set_handleriiPFiPN6swoole7ReactorEPNS_5EventEE"]
    fn swoole_event_set_handler(fd_type: c_int, event: c_int, handler: Handler);
    #[link_name = "_ZN6swoole7network6Socket8get_nameEv"]
    fn socket_get_name(socket: *mut Record) -> c_int;
    #[link_name = "_ZN6swoole7network6Socket13get_peer_nameEPNS0_7AddressE"]
    fn socket_get_peer_name(socket: *const Record, address: *mut Address) -> c_int;
    #[link_name = "_ZN6swoole7network6Socket4bindERKNS0_7AddressE"]
    fn socket_bind(socket: *mut Record, address: *const Address) -> c_int;
    #[link_name = "_ZN6swoole7network6Socket6listenEi"]
    fn socket_listen(socket: *mut Record, backlog: c_int) -> c_int;
    #[link_name = "_ZN6swoole7network6Socket6acceptEv"]
    fn socket_accept(socket: *mut Record) -> *mut Record;
    #[link_name = "_ZNK6swoole7network6Socket3dupEv"]
    fn socket_dup(socket: *const Record) -> *mut Record;
    #[link_name = "_ZN6swoole7network6Socket14set_tcp_nopushEi"]
    fn socket_set_tcp_nopush(socket: *mut Record, nopush: c_int) -> c_int;
    #[link_name = "_ZN6swoole7network6Socket15set_tcp_nodelayEi"]
    fn socket_set_tcp_nodelay(socket: *mut Record, nodelay: c_int) -> bool;
    #[link_name = "_ZN6swoole7network6Socket11set_timeoutEdi"]
    fn socket_set_timeout(socket: *mut Record, timeout: f64, kind: c_int);
    #[link_name = "_ZN6swoole7network6Socket17catch_write_errorEi"]
    fn socket_catch_write_error(socket: *mut Record, error: c_int) -> c_int;
    #[link_name = "_ZN6swoole7network6Socket22catch_write_pipe_errorEi"]
    fn socket_catch_write_pipe_error(socket: *mut Record, error: c_int) -> c_int;
    /// `network::Socket`'s defaults, which its constructor copies the timeouts from.
    static swr_v1_socket_defaults: [f64; 4];

    fn swr_v1_socket_buffer_size(fd: c_int, recv: bool, send: bool, size: u32) -> bool;
    fn swr_v1_socket_type(domain: c_int, kind: c_int) -> c_int;
    fn swr_v1_socket_domain_and_type(
        kind: c_int,
        domain: *mut c_int,
        sock_type: *mut c_int,
    ) -> bool;
    fn swr_v1_socket_host_type(host: *const u8, length: usize) -> HostTypePlan;
    static swr_v1_socket_type_classes: [u8; 16];
}

/// `swr_v1_socket_host_type_plan`.
#[repr(C)]
struct HostTypePlan {
    socket_type: c_int,
    erase: usize,
}

#[link(name = "crypto")]
unsafe extern "C" {
    fn ERR_new();
    fn ERR_set_debug(file: *const c_char, line: c_int, func: *const c_char);
    fn ERR_set_error(lib: c_int, reason: c_int, fmt: *const c_char, ...);
    fn ERR_peek_error() -> c_ulong;
    fn ERR_get_error() -> c_ulong;
}

unsafe extern "C" {
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
    #[link_name = "_Z17swoole_event_waitv"]
    fn swoole_event_wait() -> c_int;
    fn close(fd: c_int) -> c_int;
    fn pipe(fds: *mut c_int) -> c_int;
    fn unlink(path: *const c_char) -> c_int;
    fn geteuid() -> u32;
    fn usleep(usec: u32) -> c_int;
    fn signal(signo: c_int, handler: usize) -> usize;
    fn connect(fd: c_int, address: *const c_void, length: u32) -> c_int;
    fn sendto(
        fd: c_int,
        buf: *const c_void,
        n: usize,
        flags: c_int,
        address: *const c_void,
        length: u32,
    ) -> isize;
    fn shutdown(fd: c_int, how: c_int) -> c_int;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
}

/// `ERR_put_error(lib, func, reason, __FILE__, __LINE__)`, OpenSSL 3's macro.
pub(super) fn err_put_error(lib: c_int, reason: c_int) {
    // SAFETY: OpenSSL's per-thread error queue; static strings and no format.
    unsafe {
        ERR_new();
        ERR_set_debug(
            c"socket.cpp".as_ptr(),
            c_int::try_from(line!()).unwrap(),
            c"".as_ptr(),
        );
        ERR_set_error(lib, reason, std::ptr::null());
    }
}

/// `strstr(error_str, a) != nullptr || strstr(error_str, b) != nullptr` over a non-null string.
fn reason_names(error_str: *const c_char, a: &str, b: &str) -> bool {
    // SAFETY: OpenSSL's static reason string.
    let text = unsafe { CStr::from_ptr(error_str) }.to_str().unwrap();
    text.contains(a) || text.contains(b)
}

fn ssl_get_error_reason(reason: Option<&mut c_int>) -> *const c_char {
    // SAFETY: null or a writable reason.
    unsafe { socket_ssl_get_error_reason(reason.map_or(std::ptr::null_mut(), std::ptr::from_mut)) }
}

fn type_is(kind: c_int, classes: u8) -> bool {
    // SAFETY: a static table.
    usize::try_from(kind)
        .is_ok_and(|kind| kind < 16 && unsafe { swr_v1_socket_type_classes[kind] } & classes != 0)
}

fn is_dgram(kind: c_int) -> bool {
    type_is(kind, 1 << 1)
}

fn is_stream(kind: c_int) -> bool {
    type_is(kind, 1)
}

fn is_inet6(kind: c_int) -> bool {
    type_is(kind, 1 << 3)
}

fn convert_to_type(domain: c_int, kind: c_int) -> c_int {
    // SAFETY: no preconditions.
    unsafe { swr_v1_socket_type(domain, kind) }
}

/// `Socket::convert_to_type(std::string &host)`: erases the scheme from `host`.
fn convert_host_to_type(host: &mut String) -> c_int {
    // SAFETY: `host` is readable for its length.
    let plan = unsafe { swr_v1_socket_host_type(host.as_ptr(), host.len()) };
    host.drain(..plan.erase);
    plan.socket_type
}

fn get_domain_and_type(kind: c_int, domain: &mut c_int, sock_type: &mut c_int) -> c_int {
    // SAFETY: two writable ints.
    if unsafe { swr_v1_socket_domain_and_type(kind, domain, sock_type) } {
        SW_OK
    } else {
        SW_ERR
    }
}

pub(super) fn make_socket(kind: c_int, fd_type: c_int, flags: c_int) -> Option<Socket> {
    // SAFETY: no preconditions.
    Socket::from(unsafe { make_socket3(kind, fd_type, flags) })
}

fn make_server_socket(kind: c_int, address: &str, port: c_int, backlog: c_int) -> Option<Socket> {
    let address = std::ffi::CString::new(address).unwrap();
    // SAFETY: a NUL-terminated address.
    Socket::from(unsafe { make_server_socket4(kind, address.as_ptr(), port, backlog) })
}

fn unlink_path(path: &CStr) {
    // SAFETY: a NUL-terminated path.
    unsafe { unlink(path.as_ptr()) };
}

fn random_string(buf: &mut [u8]) {
    // SAFETY: `buf` is writable for its length.
    unsafe { swoole_random_string(buf.as_mut_ptr().cast(), buf.len()) };
}

/// A `network::Socket *`; the cases share it across threads as upstream's lambdas do.
#[derive(Clone, Copy)]
pub(super) struct Socket(*mut Record);

// SAFETY: each case uses a socket from one thread at a time, as upstream's do.
unsafe impl Send for Socket {}

impl Socket {
    fn from(record: *mut Record) -> Option<Self> {
        (!record.is_null()).then_some(Self(record))
    }

    fn record(&self) -> &mut Record {
        // SAFETY: a live record.
        unsafe { &mut *self.0 }
    }

    pub(super) fn free(self) {
        // SAFETY: frees the live socket once.
        unsafe { socket_free(self.0) };
    }

    /// `delete sock`: operator delete, without closing.
    fn delete(self) {
        // SAFETY: the live record, deallocated once.
        unsafe { free(self.0.cast()) };
    }

    fn get_fd(&self) -> c_int {
        self.record().fd
    }

    fn move_fd(&self) -> c_int {
        std::mem::replace(&mut self.record().fd, -1)
    }

    fn get_port(&self) -> c_int {
        self.record().info.get_port()
    }

    fn set_timeout(&self, timeout: f64, kind: c_int) {
        // SAFETY: a live socket.
        unsafe { socket_set_timeout(self.0, timeout, kind) };
    }

    fn set_block(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { socket_set_fd_option(self.0, 0, -1) }
    }

    fn set_nonblock(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { socket_set_fd_option(self.0, 1, -1) }
    }

    fn connect_sync(&self, address: &Address) -> c_int {
        // SAFETY: a live socket and address.
        unsafe { socket_connect_sync(self.0, address) }
    }

    pub(super) fn connect_address(&self, address: &Address) -> c_int {
        // SAFETY: the socket's fd and the assigned sockaddr.
        unsafe { connect(self.get_fd(), address.addr.as_ptr().cast(), address.len) }
    }

    /// `connect(host, port)`: assigns with the socket's type, then connect(2).
    fn connect(&self, host: &str, port: c_int) -> c_int {
        let mut address = Address::new();
        address.assign(self.record().socket_type, host, port, true);
        self.connect_address(&address)
    }

    fn get_name(&self) -> c_int {
        // SAFETY: a live socket.
        unsafe { socket_get_name(self.0) }
    }

    fn get_peer_name(&self, address: &mut Address) -> c_int {
        // SAFETY: a live socket and a writable address.
        unsafe { socket_get_peer_name(self.0, address) }
    }

    /// `bind(host, port)`: assigns without resolving, then binds.
    fn bind(&self, host: &str, port: c_int) -> c_int {
        let mut address = Address::new();
        if !address.assign(self.record().socket_type, host, port, false) {
            return SW_ERR;
        }
        // SAFETY: a live socket and the assigned address.
        unsafe { socket_bind(self.0, &address) }
    }

    fn listen(&self, backlog: c_int) -> c_int {
        // SAFETY: a live socket.
        unsafe { socket_listen(self.0, backlog) }
    }

    fn accept(&self) -> Option<Self> {
        // SAFETY: a live listener.
        Self::from(unsafe { socket_accept(self.0) })
    }

    fn dup(&self) -> Option<Self> {
        // SAFETY: a live socket.
        Self::from(unsafe { socket_dup(self.0) })
    }

    fn recv(&self, buf: &mut [u8], flags: c_int) -> isize {
        // SAFETY: `buf` is writable for its length.
        unsafe { socket_recv(self.0, buf.as_mut_ptr().cast(), buf.len(), flags) }
    }

    fn send(&self, buf: &[u8], flags: c_int) -> isize {
        // SAFETY: `buf` is readable for its length.
        unsafe { socket_send(self.0, buf.as_ptr().cast(), buf.len(), flags) }
    }

    fn peek(&self, buf: &mut [u8], flags: c_int) -> isize {
        // SAFETY: `buf` is writable for its length.
        unsafe { socket_peek(self.0, buf.as_mut_ptr().cast(), buf.len(), flags) }
    }

    fn recv_sync(&self, buf: &mut [u8], flags: c_int) -> isize {
        // SAFETY: `buf` is writable for its length.
        unsafe { socket_recv_sync(self.0, buf.as_mut_ptr().cast(), buf.len(), flags) }
    }

    fn send_async(&self, buf: &[u8]) -> isize {
        // SAFETY: `buf` is readable for its length.
        unsafe { socket_send_async(self.0, buf.as_ptr().cast(), buf.len()) }
    }

    fn read_sync(&self, buf: &mut [u8]) -> isize {
        // SAFETY: `buf` is writable for its length.
        unsafe { socket_read_sync(self.0, buf.as_mut_ptr().cast(), buf.len()) }
    }

    fn write_sync(&self, buf: &[u8]) -> isize {
        // SAFETY: `buf` is readable for its length.
        unsafe { socket_write_sync(self.0, buf.as_ptr().cast(), buf.len()) }
    }

    /// `sendto(host, port, data, len)`: assigns with the socket's type, then sendto(2).
    fn sendto(&self, host: &str, port: c_int, data: &[u8]) -> isize {
        let mut address = Address::new();
        if !address.assign(self.record().socket_type, host, port, true) {
            return SW_ERR as isize;
        }
        self.sendto_address(&address, data)
    }

    fn sendto_address(&self, address: &Address, data: &[u8]) -> isize {
        // SAFETY: `data` and the assigned sockaddr.
        unsafe {
            sendto(
                self.get_fd(),
                data.as_ptr().cast(),
                data.len(),
                0,
                address.addr.as_ptr().cast(),
                address.len,
            )
        }
    }

    fn sendto_sync(&self, address: &Address, data: &[u8]) -> isize {
        // SAFETY: `data` and the assigned address.
        unsafe { socket_sendto_sync(self.0, address, data.as_ptr().cast(), data.len(), 0) }
    }

    /// `recvfrom(buf, len, flags, &sa)`: recvfrom(2), retried on `EINTR` up to
    /// `SW_SOCKET_RETRY_COUNT` times.
    fn recvfrom(&self, buf: &mut [u8], address: &mut Address) -> isize {
        address.len = u32::try_from(size_of_val(&address.addr)).unwrap();
        let mut received = -1;
        for _ in 0..SW_SOCKET_RETRY_COUNT {
            // SAFETY: `buf` and the address are writable.
            received = unsafe {
                recvfrom(
                    self.get_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    0,
                    address.addr.as_mut_ptr().cast(),
                    &mut address.len,
                )
            };
            if received >= 0 || errno() != EINTR {
                break;
            }
        }
        received
    }

    fn recvfrom_sync(&self, buf: &mut [u8], address: &mut Address) -> isize {
        // SAFETY: `buf` and the address are writable.
        unsafe { socket_recvfrom_sync(self.0, buf.as_mut_ptr().cast(), buf.len(), 0, address) }
    }

    fn sendfile_sync(&self, file: &str, offset: i64, length: usize) -> c_int {
        let file = std::ffi::CString::new(file).unwrap();
        // SAFETY: a NUL-terminated path.
        unsafe { socket_sendfile_sync(self.0, file.as_ptr(), offset, length) }
    }

    fn clean(&self) {
        // SAFETY: a live socket.
        unsafe { socket_clean(self.0) };
    }

    fn check_liveness(&self) -> bool {
        // SAFETY: a live socket.
        unsafe { socket_check_liveness(self.0) }
    }

    fn shutdown(&self, how: c_int) -> c_int {
        // SAFETY: the socket's fd.
        unsafe { shutdown(self.get_fd(), how) }
    }

    fn get_out_buffer_length(&self) -> u32 {
        let buffer = self.record().out_buffer;
        // SAFETY: a live output buffer.
        if buffer.is_null() {
            0
        } else {
            unsafe { buffer_length(buffer) }
        }
    }

    fn catch_write_error(&self, error: c_int) -> c_int {
        // SAFETY: a live socket.
        unsafe { socket_catch_write_error(self.0, error) }
    }

    fn catch_write_pipe_error(&self, error: c_int) -> c_int {
        // SAFETY: a live socket.
        unsafe { socket_catch_write_pipe_error(self.0, error) }
    }
}

/// `network::Socket sock{}`: a constructed record on the stack.
fn stack_socket() -> Box<MaybeUninit<Record>> {
    // SAFETY: every field is an integer, float, bool or pointer, valid zeroed.
    let mut record = Box::new(MaybeUninit::<Record>::zeroed());
    // SAFETY: the zeroed record; the defaults only change through their setters.
    unsafe { (*record.as_mut_ptr()).timeouts = swr_v1_socket_defaults };
    record
}

fn connect_sync() {
    let mut sa = Address::new();

    let sock = make_socket(SW_SOCK_UNIX_STREAM, SW_FD_STREAM, 0).expect("socket");
    sa.assign(SW_SOCK_UNIX_STREAM, "/tmp/swole-not-exists.sock", 0, true);
    sock.set_timeout(0.3, SW_TIMEOUT_CONNECT);
    assert_eq!(sock.connect_sync(&sa), SW_ERR);
    assert_eq!(last_error(), ENOENT);
    sock.free();

    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM, 0).expect("socket");
    sa.assign(SW_SOCK_TCP, "192.168.199.199", 80, true);
    sock.set_timeout(0.3, SW_TIMEOUT_CONNECT);
    assert_eq!(sock.connect_sync(&sa), SW_ERR);
    assert_eq!(last_error(), ETIMEDOUT);
    sock.free();

    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM, 0).expect("socket");
    sa.assign(SW_SOCK_TCP, "127.0.0.1", 59999, true);
    sock.set_timeout(0.3, SW_TIMEOUT_CONNECT);
    assert_eq!(sock.connect_sync(&sa), SW_ERR);
    assert_eq!(last_error(), ECONNREFUSED);
    sock.free();

    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM, 0).expect("socket");
    sa.assign(SW_SOCK_TCP, TEST_HTTP_DOMAIN.to_str().unwrap(), 80, true);
    sock.set_timeout(0.3, SW_TIMEOUT_CONNECT);
    assert_eq!(sock.connect_sync(&sa), SW_OK);
    sock.free();

    let sock = make_socket(SW_SOCK_UDP, SW_FD_STREAM, 0).expect("socket");
    sa.assign(SW_SOCK_UDP, "127.0.0.1", 9900, true);
    sock.set_timeout(0.3, SW_TIMEOUT_CONNECT);
    assert_eq!(sock.connect_sync(&sa), SW_OK);
    sock.free();
}

fn fail() {
    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM, 0).expect("socket");

    let mut sa = Address::new();
    sa.assign(SW_SOCK_TCP, TEST_HTTP_DOMAIN.to_str().unwrap(), 80, true);
    sock.set_timeout(0.3, SW_TIMEOUT_CONNECT);
    assert_eq!(sock.connect_sync(&sa), SW_OK);

    // SAFETY: the socket's fd, closed under it as upstream does.
    unsafe { close(sock.get_fd()) };

    assert_eq!(sock.get_name(), -1);
    assert_eq!(errno(), EBADF);

    let mut peer = Address::new();
    assert_eq!(sock.get_peer_name(&mut peer), -1);
    assert_eq!(errno(), EBADF);

    // SAFETY: a live socket.
    assert_eq!(unsafe { socket_set_tcp_nopush(sock.0, 1) }, -1);
    assert_eq!(sock.listen(1), -1);

    let fd = sock.get_fd();
    // SAFETY: no preconditions.
    unsafe {
        assert!(!swr_v1_socket_buffer_size(fd, true, true, 1));
        assert!(!swr_v1_socket_buffer_size(fd, true, false, 1));
        assert!(!swr_v1_socket_buffer_size(fd, false, true, 1));
    }

    // SAFETY: a live socket.
    unsafe {
        assert!(!socket_set_tcp_nodelay(sock.0, 1));
        assert!(!socket_cork(sock.0));
        assert!(!socket_uncork(sock.0));

        assert!(!socket_set_kernel_read_timeout(sock.0, 0.1));
        assert!(!socket_set_kernel_write_timeout(sock.0, 0.1));
    }

    sock.move_fd();
    sock.free();
}

fn ssl_fail() {
    // SAFETY: sysv_signal(SIGPIPE, SIG_IGN).
    unsafe { signal(SIGPIPE, SIG_IGN) };
    let client = Client::new(SW_SOCK_TCP, false).expect("client");
    client.enable_ssl_encrypt();

    assert_eq!(client.connect(TEST_DOMAIN_BAIDU, 443, -1.0, 0), 0);
    assert_eq!(client.shutdown(SHUT_WR), 0);

    let sock = Socket(client.socket().cast());
    // SAFETY: the client's socket; TEST_STR without its NUL.
    assert_eq!(
        unsafe { socket_ssl_send(sock.0, TEST_STR.as_ptr().cast(), TEST_STR.len()) },
        SW_ERR as isize
    );
    assert_eq!(errno(), SW_ERROR_SSL_RESET);

    assert_eq!(client.shutdown(SHUT_RD), 0);

    let mut buf = [0_u8; 1024];
    // SAFETY: clears errno.
    unsafe { *errno_location() = 0 };
    // SAFETY: `buf` is writable; SW_STRL(buf) is its size less one.
    assert_eq!(
        unsafe { socket_ssl_recv(sock.0, buf.as_mut_ptr().cast(), buf.len() - 1) },
        0
    );
    assert_eq!(errno(), 0);
    // SAFETY: the client's fd.
    assert_eq!(unsafe { close(sock.get_fd()) }, 0);
    sock.move_fd();

    // SAFETY: as above.
    assert_eq!(
        unsafe { socket_ssl_recv(sock.0, buf.as_mut_ptr().cast(), buf.len() - 1) },
        0
    );
}

unsafe extern "C" {
    #[link_name = "__errno_location"]
    fn errno_location() -> *mut c_int;
    fn recvfrom(
        fd: c_int,
        buf: *mut c_void,
        n: usize,
        flags: c_int,
        address: *mut c_void,
        length: *mut u32,
    ) -> isize;
}

/// Linux's `EINTR`.
const EINTR: c_int = 4;

fn unix_dgram_pair() -> (Socket, Socket) {
    unlink_path(c"/tmp/udp_unix1.sock");
    unlink_path(c"/tmp/udp_unix2.sock");

    let sock1 = make_socket(SW_SOCK_UNIX_DGRAM, SW_FD_DGRAM_SERVER, 0).expect("socket");
    sock1.bind("/tmp/udp_unix1.sock", 0);

    let sock2 = make_socket(SW_SOCK_UNIX_DGRAM, SW_FD_DGRAM_SERVER, 0).expect("socket");
    sock2.bind("/tmp/udp_unix2.sock", 0);
    (sock1, sock2)
}

fn free_unix_dgram_pair(sock1: Socket, sock2: Socket) {
    sock1.free();
    sock2.free();
    unlink_path(c"/tmp/udp_unix1.sock");
    unlink_path(c"/tmp/udp_unix2.sock");
}

/// `strlen(test_data)` bytes.
fn test_data() -> &'static [u8] {
    &TEST_DATA[..TEST_DATA.len() - 1]
}

fn sendto_unix() {
    let (sock1, sock2) = unix_dgram_pair();

    assert!(sock1.sendto("/tmp/udp_unix2.sock", 0, test_data()) > 0);

    let mut buf = [0_u8; 1024];
    let mut sa = Address::new();
    sa.kind = SW_SOCK_UNIX_DGRAM;
    assert!(sock2.recvfrom(&mut buf, &mut sa) > 0);
    assert_eq!(test_data(), c_str(&buf));
    // SAFETY: the address's per-thread text.
    assert_eq!(
        unsafe { CStr::from_ptr(sa.get_addr()) },
        c"/tmp/udp_unix1.sock"
    );

    free_unix_dgram_pair(sock1, sock2);
}

fn test_sendto(sock_type: c_int) {
    let ip = if sock_type == SW_SOCK_UDP {
        "127.0.0.1"
    } else {
        "::1"
    };

    let sock1 = make_socket(sock_type, SW_FD_DGRAM_SERVER, 0).expect("socket");
    assert_eq!(sock1.bind(ip, 0), SW_OK);
    assert_eq!(sock1.get_name(), SW_OK);

    let sock2 = make_socket(sock_type, SW_FD_DGRAM_SERVER, 0).expect("socket");
    assert_eq!(sock2.bind(ip, 0), SW_OK);
    assert_eq!(sock2.get_name(), SW_OK);

    assert!(sock1.sendto(ip, sock2.get_port(), test_data()) > 0);

    let mut buf = [0_u8; 1024];
    let mut sa = Address::new();
    sa.kind = sock_type;
    assert!(sock2.recvfrom(&mut buf, &mut sa) > 0);

    assert_eq!(test_data(), c_str(&buf));
    assert_eq!(sa.get_port(), sock1.get_port());
    // SAFETY: the address's per-thread text.
    assert_eq!(
        unsafe { CStr::from_ptr(sa.get_addr()) }.to_str().unwrap(),
        ip
    );

    sock1.free();
    sock2.free();
}

fn sendto_ipv4() {
    test_sendto(SW_SOCK_UDP);
}

fn sendto_ipv6() {
    test_sendto(SW_SOCK_UDP6);
}

/// Runs `server` and `client` on two threads; `client` starts once `server` signals.
fn server_client(
    server: impl FnOnce(&dyn Fn()) + Send + 'static,
    client: impl FnOnce() + Send + 'static,
) {
    let (ready, wait) = mpsc::channel();
    let t1 = thread::spawn(move || server(&|| ready.send(()).unwrap()));
    let t2 = thread::spawn(move || {
        wait.recv().unwrap();
        client();
    });
    t1.join().unwrap();
    t2.join().unwrap();
}

fn recv() {
    let port = get_random_port();
    server_client(
        move |unlock| {
            let svr = make_server_socket(SW_SOCK_TCP, TEST_HOST, port, SW_BACKLOG).expect("server");
            let mut buf = [0_u8; 1024];
            svr.set_block();
            unlock();

            let client_sock = svr.accept().expect("accepted");
            client_sock.recv(&mut buf, 0);

            assert_eq!(test_data(), c_str(&buf));
            svr.free();
        },
        move || {
            let cli = make_socket(SW_SOCK_TCP, SW_FD_STREAM_CLIENT, 0).expect("socket");
            assert_eq!(cli.connect(TEST_HOST, port), SW_OK);
            thread::sleep(Duration::from_millis(100));
            cli.send(TEST_DATA, 0);
            cli.free();
        },
    );
}

fn recvfrom_sync() {
    let port = get_random_port();
    server_client(
        move |unlock| {
            let svr = make_server_socket(SW_SOCK_UDP, TEST_HOST, port, SW_BACKLOG).expect("server");
            let mut addr = Address::new();
            let mut buf = [0_u8; 1024];
            svr.set_nonblock();
            unlock();
            svr.recvfrom_sync(&mut buf, &mut addr);
            assert_eq!(test_data(), c_str(&buf));
            svr.free();
        },
        move || {
            let cli = make_socket(SW_SOCK_UDP, SW_FD_STREAM_CLIENT, 0).expect("socket");
            let mut addr = Address::new();
            addr.assign(SW_SOCK_TCP, TEST_HOST, port, true);
            assert_eq!(cli.connect_address(&addr), SW_OK);
            thread::sleep(Duration::from_millis(100));
            cli.send(TEST_DATA, 0);
            cli.free();
        },
    );
}

fn http_domain() -> &'static str {
    TEST_HTTP_DOMAIN.to_str().unwrap()
}

fn send_async_1() {
    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM_CLIENT, 0).expect("socket");
    assert!(sock.set_block());
    assert_eq!(sock.connect(http_domain(), 80), SW_OK);

    let req = http_get_request(TEST_HTTP_DOMAIN, "/");
    assert_eq!(sock.send_async(&req), req.len() as isize);

    // sw_tg_buffer(): the thread's SW_STACK_BUFFER_SIZE buffer.
    let mut buf = vec![0_u8; SW_STACK_BUFFER_SIZE];
    let n = sock.recv_sync(&mut buf, 0);
    assert!(n > 0);
    assert!(contains(&buf[..n as usize], TEST_HTTP_EXPECT));

    sock.free();
}

unsafe extern "C" fn send_async_2_read(_: *mut c_void, event: *mut Event) -> c_int {
    // SAFETY: the reactor's event and its socket.
    let socket = Socket(unsafe { (*event).socket });
    let mut buf = vec![0_u8; SW_STACK_BUFFER_SIZE];
    let n = socket.recv_sync(&mut buf, 0);
    assert!(n > 0);
    assert!(contains(&buf[..n as usize], TEST_HTTP_EXPECT));
    0
}

fn send_async_2() {
    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM_CLIENT, 0).expect("socket");
    assert!(sock.set_block());
    assert_eq!(sock.connect(http_domain(), 80), SW_OK);

    // SAFETY: a new loop on this thread.
    unsafe { swoole_event_init(SW_EVENTLOOP_WAIT_EXIT) };

    let req = http_get_request(TEST_HTTP_DOMAIN, "/");
    assert_eq!(sock.send_async(&req), req.len() as isize);

    // SAFETY: the live loop; the handler reads the socket once.
    unsafe {
        swoole_event_set_handler(SW_FD_STREAM_CLIENT, SW_EVENT_READ, send_async_2_read);
        swoole_event_add(sock.0, SW_EVENT_READ | SW_EVENT_ONCE);
        swoole_event_wait();
    }

    sock.free();
}

fn sendfile_sync() {
    let file = std::env::current_dir().unwrap().display().to_string() + "/examples/test.jpg";
    let port = get_random_port();

    let str = std::fs::read(&file).unwrap();
    let server_str = str.clone();

    server_client(
        move |unlock| {
            let svr = make_server_socket(SW_SOCK_TCP, TEST_HOST, port, SW_BACKLOG).expect("server");
            unlock();
            let cli = svr.accept().expect("accepted");
            let mut len = [0_u8; 4];
            cli.recv_sync(&mut len, MSG_WAITALL);
            let len = u32::from_be_bytes(len) as usize;
            assert_eq!(len, server_str.len());
            assert!(len < 1024 * 1024);
            let mut data = vec![0_u8; len];
            cli.recv_sync(&mut data, MSG_WAITALL);
            assert_eq!(c_str(&data), c_str(&server_str));
            cli.free();
            svr.free();
        },
        move || {
            let cli = make_socket(SW_SOCK_TCP, SW_FD_STREAM_CLIENT, 0).expect("socket");
            let mut addr = Address::new();
            addr.assign(SW_SOCK_TCP, TEST_HOST, port, true);
            assert_eq!(cli.connect_address(&addr), SW_OK);
            let len = u32::try_from(str.len()).unwrap().to_be_bytes();
            cli.send(&len, 0);
            assert_eq!(cli.sendfile_sync(&file, 0, 0), SW_OK);
            cli.free();
        },
    );
}

fn sendfile() {
    let file = "/tmp/swoole-file-not-exists";
    let cli = make_socket(SW_SOCK_TCP, SW_FD_STREAM_CLIENT, 0).expect("socket");
    let mut addr = Address::new();
    addr.assign(SW_SOCK_TCP, http_domain(), 80, true);
    assert_eq!(cli.connect_address(&addr), SW_OK);

    assert_eq!(cli.sendfile_sync(file, 0, 0), SW_ERR);
    assert_eq!(errno(), ENOENT);

    // File fp(file, File::WRITE | File::CREATE).
    let mut fp = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(file);
    assert!(fp.is_ok());

    assert_eq!(cli.sendfile_sync(file, 0, 0), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_FILE_EMPTY);

    std::io::Write::write_all(fp.as_mut().unwrap(), TEST_STR).unwrap();
    drop(fp);

    assert_eq!(cli.sendfile_sync(file, 10, 100), SW_ERR);
    assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);

    // fp.open(file, File::WRITE | File::APPEND).
    let fp = std::fs::OpenOptions::new().append(true).open(file);
    assert!(fp.is_ok());
    let req = http_get_request(TEST_HTTP_DOMAIN, "/");
    std::io::Write::write_all(&mut fp.unwrap(), &req).unwrap();

    assert_eq!(cli.sendfile_sync(file, TEST_STR.len() as i64, 0), SW_OK);

    let mut rbuf = [0_u8; 4096];
    let n = cli.recv_sync(&mut rbuf, 0);
    assert!(n > 0);

    assert!(contains(&rbuf[..n as usize], TEST_HTTP_EXPECT));

    cli.free();

    assert!(std::fs::remove_file(file).is_ok());
}

fn peek() {
    let (sock1, sock2) = unix_dgram_pair();

    assert!(sock1.sendto("/tmp/udp_unix2.sock", 0, test_data()) > 0);

    let mut buf = [0_u8; 1024];
    assert!(sock2.peek(&mut buf, 0) > 0);
    assert_eq!(test_data(), c_str(&buf));

    buf.fill(0);
    assert!(sock2.recv(&mut buf, 0) > 0);
    assert_eq!(test_data(), c_str(&buf));

    free_unix_dgram_pair(sock1, sock2);
}

fn sendto_sync() {
    let (sock1, sock2) = unix_dgram_pair();

    let mut sendbuf = vec![0_u8; 65536];
    random_string(&mut sendbuf[..65535]);
    let sent = sendbuf.clone();

    let t1 = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        let mut recvbuf = vec![0_u8; 65536];
        loop {
            let retval = sock2.recv(&mut recvbuf[..65535], 0);
            recvbuf[retval as usize] = 0;
            if retval == 3 {
                assert_eq!(c_str(&recvbuf), b"end");
                break;
            }
            assert_eq!(c_str(&sent), c_str(&recvbuf));
        }
    });

    let mut sock2_addr = Address::new();
    assert!(sock2_addr.assign(SW_SOCK_UNIX_DGRAM, "/tmp/udp_unix2.sock", 0, true));

    for _ in 0..10 {
        assert!(sock1.sendto_sync(&sock2_addr, c_str(&sendbuf)) > 0);
    }
    assert!(sock1.sendto_sync(&sock2_addr, b"end") > 0);

    t1.join().unwrap();

    free_unix_dgram_pair(sock1, sock2);
}

fn clean() {
    let (sock1, sock2) = unix_dgram_pair();

    let mut sendbuf = vec![0_u8; 65536];
    random_string(&mut sendbuf[..65535]);

    let mut sock2_addr = Address::new();
    assert!(sock2_addr.assign(SW_SOCK_UNIX_DGRAM, "/tmp/udp_unix2.sock", 0, true));

    for _ in 0..3 {
        assert!(sock1.sendto_address(&sock2_addr, c_str(&sendbuf)) > 0);
    }

    sock2.clean();
    let mut recvbuf = [0_u8; 1024];
    let retval = sock2.peek(&mut recvbuf, MSG_DONTWAIT);
    assert_eq!(retval, -1);

    free_unix_dgram_pair(sock1, sock2);
}

fn check_liveness() {
    // TEST_PORT + __LINE__, upstream's line.
    let svr_port = TEST_PORT + 487;
    server_client(
        move |unlock| {
            let svr =
                make_server_socket(SW_SOCK_TCP, TEST_HOST, svr_port, SW_BACKLOG).expect("server");
            unlock();

            let cli = svr.accept();
            assert!(cli.is_some());
            let cli = cli.unwrap();

            let mut buf = [0_u8; 1024];
            cli.recv(&mut buf, 0);
            assert_eq!(test_data(), c_str(&buf));

            let n = cli.recv(&mut buf, 0);
            buf[n as usize] = 0;
            assert_eq!(c_str(&buf), b"close");
            cli.shutdown(SHUT_RDWR);
            cli.free();

            svr.free();
        },
        move || {
            let cli = make_socket(SW_SOCK_TCP, SW_FD_STREAM_CLIENT, 0).expect("socket");
            assert_eq!(cli.connect(TEST_HOST, svr_port), SW_OK);

            cli.send(TEST_DATA, 0);
            thread::sleep(Duration::from_millis(1));
            assert!(cli.check_liveness());

            cli.send(b"close", 0);
            thread::sleep(Duration::from_millis(5));
            assert!(!cli.check_liveness());

            cli.free();
        },
    );
}

fn test_socket_sync(sock: Socket, connect: bool) {
    if connect {
        let mut addr = Address::new();
        assert!(addr.assign_url(&format!("tcp://{}:80", http_domain())));
        assert_eq!(sock.connect_address(&addr), 0);
    }

    let req = http_get_request(TEST_HTTP_DOMAIN, "/get");
    assert_eq!(sock.write_sync(&req), req.len() as isize);
    assert!(sock.check_liveness());

    let mut resp = Vec::new();
    loop {
        let mut buf = [0_u8; 1024];
        let n = sock.read_sync(&mut buf);
        if n == 0 {
            break;
        }
        assert!(n > 0);
        resp.extend_from_slice(&buf[..n as usize]);
    }

    assert!(contains(&resp, TEST_HTTP_EXPECT));

    // SAFETY: no preconditions.
    unsafe { usleep(50000) };
    assert!(!sock.check_liveness());

    sock.free();
}

fn sync() {
    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM, 0).expect("socket");
    test_socket_sync(sock, true);
}

fn dup() {
    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM, 0).expect("socket");
    let mut addr = Address::new();
    assert!(addr.assign_url(&format!("tcp://{}:80", http_domain())));
    assert_eq!(sock.connect_address(&addr), 0);

    let sock_2 = sock.dup().expect("duplicated");
    sock.free();

    test_socket_sync(sock_2, false);
}

fn convert_to_type_case() {
    assert_eq!(convert_to_type(AF_INET, SOCK_STREAM), SW_SOCK_TCP);
    assert_eq!(convert_to_type(AF_INET6, SOCK_STREAM), SW_SOCK_TCP6);
    assert_eq!(convert_to_type(AF_INET, SOCK_DGRAM), SW_SOCK_UDP);
    assert_eq!(convert_to_type(AF_INET6, SOCK_DGRAM), SW_SOCK_UDP6);
    assert_eq!(convert_to_type(AF_LOCAL, SOCK_STREAM), SW_SOCK_UNIX_STREAM);
    assert_eq!(convert_to_type(AF_LOCAL, SOCK_DGRAM), SW_SOCK_UNIX_DGRAM);
    assert_eq!(convert_to_type(AF_INET, SOCK_RAW), SW_SOCK_RAW);
    assert_eq!(convert_to_type(AF_INET6, SOCK_RAW), SW_SOCK_RAW6);

    let mut s1 = String::from("unix:///tmp/swoole.sock");
    assert_eq!(convert_host_to_type(&mut s1), SW_SOCK_UNIX_STREAM);
    assert_eq!(s1, "/tmp/swoole.sock");

    let mut s2 = String::from("127.0.0.1");
    assert_eq!(convert_host_to_type(&mut s2), SW_SOCK_TCP);

    let mut s3 = String::from("::1");
    assert_eq!(convert_host_to_type(&mut s3), SW_SOCK_TCP6);

    let mut s4 = String::from("unix:/tmp/swoole.sock");
    assert_eq!(convert_host_to_type(&mut s4), SW_SOCK_UNIX_STREAM);
    assert_eq!(s4, "/tmp/swoole.sock");
}

fn test_sock_type(kind: c_int, expect_sock_domain: c_int, expect_sock_type: c_int) {
    let (mut sock_domain, mut sock_type) = (0, 0);
    assert_eq!(
        get_domain_and_type(kind, &mut sock_domain, &mut sock_type),
        SW_OK
    );
    assert_eq!(sock_domain, expect_sock_domain);
    assert_eq!(sock_type, expect_sock_type);
}

fn get_domain_and_type_case() {
    test_sock_type(SW_SOCK_TCP, AF_INET, SOCK_STREAM);
    test_sock_type(SW_SOCK_TCP6, AF_INET6, SOCK_STREAM);
    test_sock_type(SW_SOCK_UDP, AF_INET, SOCK_DGRAM);
    test_sock_type(SW_SOCK_UDP6, AF_INET6, SOCK_DGRAM);
    test_sock_type(SW_SOCK_UNIX_STREAM, AF_LOCAL, SOCK_STREAM);
    test_sock_type(SW_SOCK_UNIX_DGRAM, AF_LOCAL, SOCK_DGRAM);
    test_sock_type(SW_SOCK_RAW, AF_INET, SOCK_RAW);
    test_sock_type(SW_SOCK_RAW6, AF_INET6, SOCK_RAW);

    assert!(is_dgram(SW_SOCK_UDP6));
    assert!(is_stream(SW_SOCK_TCP));

    let (mut sock_domain, mut sock_type) = (0, 0);
    assert_eq!(
        get_domain_and_type(SW_SOCK_RAW6 + 1, &mut sock_domain, &mut sock_type),
        SW_ERR
    );
}

fn make_socket_case() {
    let sock = make_socket(SW_SOCK_RAW, SW_FD_STREAM, 0);
    assert!(sock.is_none());
    assert_eq!(errno(), EPROTONOSUPPORT);
    assert_eq!(last_error(), EPROTONOSUPPORT);

    // SAFETY: no preconditions.
    let sock = unsafe { make_socket6(SW_SOCK_TCP, SW_FD_STREAM, AF_INET6, SOCK_RDM, 999, 0) };
    assert!(sock.is_null());
    assert_eq!(errno(), EINVAL);
    assert_eq!(last_error(), EINVAL);
}

fn make_server_socket_case() {
    let bad_addr = "199.199.0.0";

    let sock = make_server_socket(SW_SOCK_RAW, bad_addr, 0, SW_BACKLOG);
    assert!(sock.is_none());
    // SAFETY: no preconditions.
    if unsafe { geteuid() } == 0 {
        // root
        assert_eq!(errno(), EPROTONOSUPPORT);
        assert_eq!(last_error(), EPROTONOSUPPORT);
    } else {
        assert_eq!(errno(), ESOCKTNOSUPPORT);
        assert_eq!(last_error(), ESOCKTNOSUPPORT);
    }

    let sock = make_server_socket(SW_SOCK_TCP, bad_addr, 0, SW_BACKLOG);
    assert!(sock.is_none());
    assert_eq!(errno(), EADDRNOTAVAIL);

    let sock = make_server_socket(SW_SOCK_TCP, TEST_HOST, 0, -1);
    assert!(sock.is_some());
    sock.unwrap().free();
}

fn make_server_socket_rebind_after_time_wait() {
    let server = make_server_socket(SW_SOCK_TCP, TEST_HOST, 0, SW_BACKLOG);
    assert!(server.is_some());
    let server = server.unwrap();
    let port = server.get_port();
    assert!(port > 0);

    let mut address = Address::new();
    assert!(address.assign(SW_SOCK_TCP, TEST_HOST, port, true));
    let client = make_socket(SW_SOCK_TCP, SW_FD_STREAM, 0);
    assert!(client.is_some());
    let client = client.unwrap();
    assert_eq!(client.connect_sync(&address), SW_OK);

    let connection = server.accept();
    assert!(connection.is_some());
    // The server endpoint must close first so TIME_WAIT lands on the listener port. If the client closed first, it
    // would land on the client's ephemeral port and this test would pass without the fix.
    connection.unwrap().free();

    let mut buf = [0_u8; 1];
    assert_eq!(client.recv_sync(&mut buf, 0), 0);
    client.free();
    server.free();

    let server = make_server_socket(SW_SOCK_TCP, TEST_HOST, port, SW_BACKLOG);
    assert!(server.is_some());
    server.unwrap().free();
}

fn ssl_get_error_reason_case() {
    // SAFETY: no preconditions.
    unsafe { swoole_ssl_init() };
    {
        let mut reason = -1;
        let error_str = ssl_get_error_reason(Some(&mut reason));

        assert!(error_str.is_null());
        assert_eq!(reason, 0);
    }
    // One error.
    {
        err_put_error(ERR_LIB_SSL, SSL_R_SSLV3_ALERT_CERTIFICATE_EXPIRED);

        let mut reason = -1;
        let error_str = ssl_get_error_reason(Some(&mut reason));

        assert_eq!(reason, SSL_R_SSLV3_ALERT_CERTIFICATE_EXPIRED);

        assert!(!error_str.is_null());
        assert!(reason_names(
            error_str,
            "certificate expired",
            "CERTIFICATE_EXPIRED"
        ));

        // ERR_get_error removed the error.
        // SAFETY: no preconditions.
        assert_eq!(unsafe { ERR_peek_error() }, 0);
    }

    // Several errors: the first comes back.
    {
        err_put_error(ERR_LIB_SSL, SSL_R_SSLV3_ALERT_BAD_CERTIFICATE);
        err_put_error(ERR_LIB_SSL, SSL_R_PROTOCOL_IS_SHUTDOWN);

        let mut reason = -1;
        let error_str = ssl_get_error_reason(Some(&mut reason));

        assert_eq!(reason, SSL_R_SSLV3_ALERT_BAD_CERTIFICATE);

        assert!(!error_str.is_null());
        assert!(reason_names(
            error_str,
            "bad certificate",
            "BAD_CERTIFICATE"
        ));

        // One error is still queued.
        // SAFETY: no preconditions.
        assert_ne!(unsafe { ERR_peek_error() }, 0);

        // SAFETY: no preconditions.
        unsafe { ERR_get_error() };
    }

    // Another library's error.
    {
        err_put_error(ERR_LIB_BIO, BIO_R_BROKEN_PIPE);

        let mut reason = -1;
        let error_str = ssl_get_error_reason(Some(&mut reason));

        assert_eq!(reason, BIO_R_BROKEN_PIPE);

        assert!(!error_str.is_null());
        assert!(reason_names(error_str, "broken pipe", "BROKEN_PIPE"));
    }

    // A null reason.
    {
        err_put_error(ERR_LIB_SSL, SSL_R_SSL_HANDSHAKE_FAILURE);

        let error_str = ssl_get_error_reason(None);

        assert!(!error_str.is_null());
        assert!(reason_names(
            error_str,
            "handshake failure",
            "HANDSHAKE_FAILURE"
        ));
    }

    // An uncommon code that may have no string: either outcome passes.
    {
        err_put_error(ERR_LIB_USER, 12345);

        let mut reason = -1;
        let _error_str = ssl_get_error_reason(Some(&mut reason));

        assert_eq!(reason, 12345);
    }

    // A second call finds the queue empty.
    {
        err_put_error(ERR_LIB_SSL, SSL_R_LIBRARY_HAS_NO_CIPHERS);

        let mut reason1 = -1;
        let error_str1 = ssl_get_error_reason(Some(&mut reason1));

        assert_eq!(reason1, SSL_R_LIBRARY_HAS_NO_CIPHERS);
        assert!(!error_str1.is_null());

        let mut reason2 = -1;
        let error_str2 = ssl_get_error_reason(Some(&mut reason2));

        assert_eq!(reason2, 0);
        assert!(error_str2.is_null());
    }
}

fn catch_error() {
    let mut fake = stack_socket();
    let fake_sock = Socket(fake.as_mut_ptr());
    assert_eq!(fake_sock.catch_write_pipe_error(ENOBUFS), SW_REDUCE_SIZE);
    assert_eq!(fake_sock.catch_write_pipe_error(EMSGSIZE), SW_REDUCE_SIZE);
    assert_eq!(fake_sock.catch_write_pipe_error(EAGAIN), SW_WAIT);

    assert_eq!(fake_sock.catch_write_error(ENOBUFS), SW_WAIT);
}

fn misc() {
    let sock = make_socket(SW_SOCK_TCP, SW_FD_STREAM, 0).expect("socket");
    assert!(sock.set_nonblock());
    assert_eq!(sock.get_out_buffer_length(), 0);
    assert!(sock.set_block());
    assert!(sock.get_fd() > 0);
    assert_eq!(sock.connect(http_domain(), 80), 0);
    assert_eq!(sock.get_name(), 0);
    assert!(sock.get_port() > 1024);

    let fd = sock.get_fd();
    let sock_fd = sock.move_fd();
    assert_eq!(fd, sock_fd);
    assert_eq!(sock.get_fd(), -1);

    assert_eq!(sock.catch_write_pipe_error(EMSGSIZE), SW_REDUCE_SIZE);
    assert!(!sock.record().kernel_nobufs);

    assert_eq!(sock.catch_write_pipe_error(ENOBUFS), SW_REDUCE_SIZE);
    assert!(sock.record().kernel_nobufs);

    assert_eq!(
        sock.catch_write_error(EAGAIN),
        sock.catch_write_pipe_error(EAGAIN)
    );

    sock.delete();

    assert!(is_dgram(SW_SOCK_UDP));
    assert!(!is_dgram(SW_SOCK_TCP6));

    assert!(is_stream(SW_SOCK_TCP));
    assert!(!is_stream(SW_SOCK_UDP));

    assert!(is_inet6(SW_SOCK_UDP6));
    assert!(!is_inet6(SW_SOCK_UDP));

    let mut fds = [0; 2];
    // SAFETY: two writable descriptors.
    assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);

    let mut sock1 = stack_socket();
    // SAFETY: the constructed record.
    unsafe { (*sock1.as_mut_ptr()).fd = fds[1] };

    let mut sock2 = stack_socket();
    // SAFETY: the constructed record.
    unsafe { (*sock2.as_mut_ptr()).fd = fds[0] };

    let data = test_data();
    // SAFETY: `write` on the record's fd, readable `data`.
    assert_eq!(
        unsafe { write((*sock1.as_ptr()).fd, data.as_ptr().cast(), data.len()) },
        data.len() as isize
    );

    let mut rbuf = [0_u8; 128];
    // SAFETY: `read` on the record's fd into a writable buffer.
    assert_eq!(
        unsafe { read((*sock2.as_ptr()).fd, rbuf.as_mut_ptr().cast(), rbuf.len()) },
        data.len() as isize
    );
    assert_eq!(data, &rbuf[..data.len()]);
}

pub(super) const CASES: super::Cases = &[
    ("connect_sync", connect_sync),
    ("fail", fail),
    ("ssl_fail", ssl_fail),
    ("sendto", sendto_unix),
    ("sendto_ipv4", sendto_ipv4),
    ("sendto_ipv6", sendto_ipv6),
    ("recv", recv),
    ("recvfrom_sync", recvfrom_sync),
    ("send_async_1", send_async_1),
    ("send_async_2", send_async_2),
    ("sendfile_sync", sendfile_sync),
    ("sendfile", sendfile),
    ("peek", peek),
    ("sendto_sync", sendto_sync),
    ("clean", clean),
    ("check_liveness", check_liveness),
    ("sync", sync),
    ("dup", dup),
    ("convert_to_type", convert_to_type_case),
    ("get_domain_and_type", get_domain_and_type_case),
    ("make_socket", make_socket_case),
    ("make_server_socket", make_server_socket_case),
    (
        "make_server_socket_rebind_after_time_wait",
        make_server_socket_rebind_after_time_wait,
    ),
    ("ssl_get_error_reason", ssl_get_error_reason_case),
    ("catch_error", catch_error),
    ("misc", misc),
];
