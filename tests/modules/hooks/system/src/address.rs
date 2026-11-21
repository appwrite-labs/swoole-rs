//! Upstream's `core-tests/src/network/address.cpp`, one Rust function per TEST with the
//! same inputs and expected values. `network::Address` is inline in the hybrid header
//! over the `swr_v1_clients_address_*` and `swr_v1_clients_url` exports; `Address`
//! here makes the same calls over the same record.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use super::network_socket::make_socket;
use super::{AF_INET6, ENOENT, errno, last_error};

const SW_SOCK_TCP: c_int = 1;
const SW_SOCK_UDP: c_int = 2;
const SW_SOCK_TCP6: c_int = 3;
const SW_SOCK_UNIX_STREAM: c_int = 5;
const SW_SOCK_UNIX_DGRAM: c_int = 6;
const SW_SOCK_RAW: c_int = 7;
const SW_SOCK_RAW6: c_int = 8;
const AF_UNIX: c_int = 1;
const AF_INET: c_int = 2;
const SW_FD_STREAM: c_int = 4;
const SW_ERR: c_int = -1;
const SW_ERROR_NAME_TOO_LONG: c_int = 504;
const SW_ERROR_DNSLOOKUP_RESOLVE_FAILED: c_int = 711;
const SW_ERROR_BAD_HOST_ADDR: c_int = 722;
const SW_ERROR_BAD_PORT: c_int = 723;
const SW_ERROR_BAD_SOCKET_TYPE: c_int = 724;
const SWR_CLIENTS_URL_INVALID: c_int = -1;
const SWR_CLIENTS_URL_PORT_RANGE: c_int = -2;
const ECONNREFUSED: c_int = 111;
const EAFNOSUPPORT: c_int = 97;
const TEST_DOMAIN_BAIDU: &str = "www.baidu.com";
const TEST_HTTP_DOMAIN: &str = "www.gov.cn";
const TEST_HOST: &str = "127.0.0.1";
const TEST_LOG_FILE: &str = "/tmp/swoole.log";

type Resolve = unsafe extern "C" fn(c_int, *const c_char, *mut c_char) -> c_int;

/// `swr_v1_clients_url_plan`.
#[repr(C)]
struct UrlPlan {
    outcome: c_int,
    kind: c_int,
    host_offset: usize,
    host_length: usize,
    port: c_int,
}

unsafe extern "C-unwind" {
    fn swr_v1_clients_address_assign(
        address: *mut c_void,
        length: *mut u32,
        kind: c_int,
        host: *const c_char,
        host_length: usize,
        port: c_int,
        resolve: Option<Resolve>,
    ) -> c_int;
    fn swr_v1_clients_url(data: *const u8, length: usize) -> UrlPlan;
    fn swr_v1_clients_address_set_port(address: *mut Address, port: c_int);
    fn swr_v1_clients_address_loopback(address: *const Address) -> bool;
    fn swr_v1_clients_address_empty(address: *const Address) -> bool;
    fn swr_v1_clients_address_type(kind: c_int) -> *const c_char;
    fn swr_v1_clients_address_string(family: c_int, address: *const c_void) -> *const c_char;
    fn swr_v1_clients_address_verify(family: c_int, host: *const c_char) -> bool;
    fn swr_v1_core_set_last_error(code: c_int);
}

unsafe extern "C" {
    fn swr_v1_clients_gethostbyname(flags: c_int, name: *const c_char, addr: *mut c_char) -> c_int;
    fn swr_v1_clients_address_port(address: *const c_void) -> c_int;
    fn swr_v1_clients_address_host(address: *const c_void) -> *const c_char;
}

unsafe extern "C" {
    fn __errno_location() -> *mut c_int;
}

/// `swoole::network::Address`: the sockaddr union, its length and the socket type.
#[repr(C)]
#[derive(Clone)]
pub(super) struct Address {
    pub(super) addr: [u32; 28],
    pub(super) len: u32,
    pub(super) kind: c_int,
}

impl Address {
    pub(super) fn new() -> Self {
        Self {
            addr: [0; 28],
            len: 0,
            kind: 0,
        }
    }

    /// `assign(type, host, port = 0, resolve_name = true)`.
    pub(super) fn assign(
        &mut self,
        kind: c_int,
        host: &str,
        port: c_int,
        resolve_name: bool,
    ) -> bool {
        self.kind = kind;
        let host = CString::new(host).unwrap();
        let resolve: Option<Resolve> = resolve_name.then_some(swr_v1_clients_gethostbyname);
        // SAFETY: the record's union and length; a NUL-terminated host.
        let error = unsafe {
            swr_v1_clients_address_assign(
                self.addr.as_mut_ptr().cast(),
                &mut self.len,
                kind,
                host.as_ptr(),
                host.as_bytes().len(),
                port,
                resolve,
            )
        };
        if error != 0 {
            // SAFETY: sets this thread's last error.
            unsafe { swr_v1_core_set_last_error(error) };
            return false;
        }
        true
    }

    /// `assign(url)`.
    pub(super) fn assign_url(&mut self, url: &str) -> bool {
        // SAFETY: `url` is readable for its length.
        let plan = unsafe { swr_v1_clients_url(url.as_ptr(), url.len()) };
        assert_ne!(
            plan.outcome, SWR_CLIENTS_URL_PORT_RANGE,
            "std::out_of_range(\"stoi\")"
        );
        if plan.outcome == SWR_CLIENTS_URL_INVALID {
            // swoole_error_log(SW_LOG_NOTICE, SW_ERROR_BAD_HOST_ADDR, ...): the notice is below the log level.
            // SAFETY: sets this thread's last error.
            unsafe { swr_v1_core_set_last_error(SW_ERROR_BAD_HOST_ADDR) };
            return false;
        }
        let host = &url[plan.host_offset..plan.host_offset + plan.host_length];
        self.assign(plan.kind, host, plan.port, true)
    }

    pub(super) fn get_port(&self) -> c_int {
        // SAFETY: a live record.
        unsafe { swr_v1_clients_address_port(std::ptr::from_ref(self).cast()) }
    }

    fn set_port(&mut self, port: c_int) {
        // SAFETY: a live record.
        unsafe { swr_v1_clients_address_set_port(self, port) }
    }

    fn is_loopback_addr(&self) -> bool {
        // SAFETY: a live record.
        unsafe { swr_v1_clients_address_loopback(self) }
    }

    pub(super) fn get_addr(&self) -> *const c_char {
        // SAFETY: a live record.
        unsafe { swr_v1_clients_address_host(std::ptr::from_ref(self).cast()) }
    }

    /// `addr.un.sun_path`, after the two-byte `sun_family`.
    fn sun_path(&mut self) -> *mut c_char {
        // SAFETY: the union is 112 bytes; sun_path starts at byte 2.
        unsafe { self.addr.as_mut_ptr().cast::<c_char>().add(2) }
    }

    fn empty(&self) -> bool {
        // SAFETY: a live record.
        unsafe { swr_v1_clients_address_empty(self) }
    }
}

fn verify_ip(family: c_int, host: &CStr) -> bool {
    // SAFETY: a NUL-terminated host.
    unsafe { swr_v1_clients_address_verify(family, host.as_ptr()) }
}

/// `CStr` view of a returned address string.
fn text<'a>(ptr: *const c_char) -> &'a CStr {
    // SAFETY: the exports return NUL-terminated text that outlives this check.
    unsafe { CStr::from_ptr(ptr) }
}

fn type_str(kind: c_int) -> &'static CStr {
    // SAFETY: a static name.
    unsafe { CStr::from_ptr(swr_v1_clients_address_type(kind)) }
}

/// `make_socket(type, SW_FD_STREAM, 0)`, `sock->connect(addr)` and `sock->free()`.
fn connect_fails(kind: c_int, urls: &[&str], expected: c_int) {
    let sock = make_socket(kind, SW_FD_STREAM, 0).expect("socket");
    let mut addr = Address::new();
    for url in urls {
        assert!(addr.assign_url(url));
        assert_eq!(sock.connect_address(&addr), SW_ERR);
        assert_eq!(errno(), expected);
    }
    sock.free();
}

fn basic() {
    let mut address = Address::new();
    assert!(address.empty());
    assert!(address.assign(SW_SOCK_TCP, TEST_DOMAIN_BAIDU, 80, true));
    address.set_port(443);
    assert_eq!(address.get_port(), 443);

    assert!(address.assign(SW_SOCK_TCP6, TEST_HTTP_DOMAIN, 80, true));
    address.set_port(9501);
    assert_eq!(address.get_port(), 9501);
}

fn dns_fail() {
    let mut address = Address::new();
    assert!(!address.assign(
        SW_SOCK_TCP,
        &format!("{TEST_DOMAIN_BAIDU}not-exists"),
        80,
        true
    ));
    assert_eq!(last_error(), SW_ERROR_DNSLOOKUP_RESOLVE_FAILED);
}

fn path_to_long() {
    let mut address = Address::new();
    // swoole::String path; path.repeat("HELLO", 5, 128);
    let path = "HELLO".repeat(128);
    assert!(!address.assign(SW_SOCK_UNIX_DGRAM, &path, 0, true));
    assert_eq!(last_error(), SW_ERROR_NAME_TOO_LONG);
}

fn bad_type() {
    let mut address = Address::new();
    assert!(!address.assign(SW_SOCK_RAW6 + 9, TEST_DOMAIN_BAIDU, 0, true));
    assert_eq!(last_error(), SW_ERROR_BAD_SOCKET_TYPE);
}

fn type_str_() {
    assert_eq!(type_str(SW_SOCK_TCP), c"IPv4");
    assert_eq!(type_str(SW_SOCK_UNIX_STREAM), c"UnixSocket");
    assert_eq!(type_str(SW_SOCK_TCP6), c"IPv6");
    assert_eq!(type_str(SW_SOCK_RAW6 + 9), c"Unknown");
}

fn is_loopback_addr() {
    let mut address = Address::new();
    assert!(address.assign(SW_SOCK_TCP, TEST_DOMAIN_BAIDU, 80, true));
    assert!(!address.is_loopback_addr());

    assert!(address.assign(SW_SOCK_TCP, TEST_HOST, 80, true));
    assert!(address.is_loopback_addr());

    assert!(address.assign(SW_SOCK_TCP6, "::1", 80, true));
    assert!(address.is_loopback_addr());

    assert!(address.assign(SW_SOCK_TCP6, TEST_HTTP_DOMAIN, 443, true));
    assert!(!address.is_loopback_addr());

    assert!(address.assign(SW_SOCK_UNIX_DGRAM, TEST_LOG_FILE, 0, true));
    assert!(!address.is_loopback_addr());
}

fn ipv4_addr() {
    connect_fails(
        SW_SOCK_TCP,
        &["tcp://127.0.0.1:12345", "tcp://localhost:12345"],
        ECONNREFUSED,
    );
}

fn ipv6_addr() {
    connect_fails(
        SW_SOCK_TCP6,
        &["tcp://[::1]:12345", "tcp://[ip6-localhost]:12345"],
        ECONNREFUSED,
    );
}

fn unix_addr() {
    connect_fails(
        SW_SOCK_UNIX_STREAM,
        &["unix:///tmp/swoole-not-exists.sock"],
        ENOENT,
    );
}

fn bad_addr() {
    let mut addr = Address::new();
    assert!(!addr.assign_url("test://[::1]:12345"));
    assert_eq!(last_error(), SW_ERROR_BAD_HOST_ADDR);

    let buf = [0u8; 16];
    // SAFETY: 16 readable bytes.
    assert!(unsafe { swr_v1_clients_address_string(AF_INET6 + 9, buf.as_ptr().cast()) }.is_null());
    assert_eq!(errno(), EAFNOSUPPORT);
}

fn bad_port() {
    let mut addr = Address::new();
    assert!(!addr.assign_url("tcp://[::1]:92345"));
    assert_eq!(last_error(), SW_ERROR_BAD_PORT);
}

fn loopback_addr() {
    let mut addr1 = Address::new();
    addr1.assign(SW_SOCK_TCP, "127.0.0.1", 0, true);
    assert!(addr1.is_loopback_addr());

    let mut addr2 = Address::new();
    addr2.assign(SW_SOCK_TCP6, "::1", 0, true);
    // Upstream asserts addr1 here, not addr2.
    assert!(addr1.is_loopback_addr());

    let mut addr3 = Address::new();
    addr3.assign(SW_SOCK_TCP, "192.168.1.2", 0, true);
    assert!(!addr3.is_loopback_addr());

    let mut addr4 = Address::new();
    addr4.assign(SW_SOCK_TCP6, "192::66::88", 0, true);
    assert!(!addr4.is_loopback_addr());
}

/// Not an upstream TEST: the record's fields, value copies and the per-thread scratch
/// `get_addr` and `verify_ip` share.
fn record() {
    let (mut v4, mut v6, mut local) = (Address::new(), Address::new(), Address::new());
    assert!(v4.empty());
    assert!(v4.assign(SW_SOCK_UDP, "127.0.0.1", 80, false));
    assert!(v6.assign(SW_SOCK_TCP6, "::1", 443, false));
    assert!(local.assign(SW_SOCK_UNIX_DGRAM, "/tmp/address-record", 0, false));
    let scratch = v4.get_addr();
    assert_eq!(text(scratch), c"127.0.0.1");
    assert_eq!(v6.get_addr(), scratch);
    assert_eq!(text(scratch), c"::1");
    assert!(verify_ip(AF_INET, c"127.0.0.1"));
    // SAFETY: the scratch holds at least the four address bytes verify_ip wrote.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(scratch.cast::<u8>(), 4) },
        [127, 0, 0, 1]
    );
    assert_eq!(v4.get_addr(), scratch);
    let other = scratch as usize;
    let v6_copy = v6.clone();
    std::thread::spawn(move || {
        assert_ne!(v6_copy.get_addr() as usize, other);
        assert!(verify_ip(AF_INET6, c"fe80::1"));
    })
    .join()
    .unwrap();
    assert_eq!(text(scratch), c"127.0.0.1");
    assert_eq!(local.get_addr(), local.sun_path().cast_const());
    let mut copy = local.clone();
    // SAFETY: byte 1 of sun_path is inside the union.
    unsafe { *local.sun_path().add(1) = b'X' as c_char };
    assert_eq!(text(copy.get_addr()), c"/tmp/address-record");
    assert!(v4.is_loopback_addr() && v6.is_loopback_addr());
    assert!(v4.assign(SW_SOCK_TCP, "127.0.0.2", 0, false));
    assert!(!v4.is_loopback_addr());
    for address in [&mut v4, &mut v6] {
        for port in [c_int::MIN, -65537, -1, 0, 1, 65535, 65536, c_int::MAX] {
            address.set_port(port);
            assert_eq!(address.get_port(), c_int::from(port as u16));
        }
    }
    copy.set_port(65535);
    assert_eq!(copy.get_port(), 0);
    assert_eq!(text(copy.get_addr()), c"/tmp/address-record");
    let mut unknown = Address::new();
    unknown.kind = 99;
    assert!(!unknown.empty() && unknown.get_port() == 0);
    assert_eq!(text(unknown.get_addr()), c"unknown");
    assert_eq!(type_str(SW_SOCK_RAW), c"IPv4");
    assert_eq!(type_str(SW_SOCK_RAW6), c"IPv6");
    assert_eq!(type_str(SW_SOCK_UNIX_STREAM), c"UnixSocket");
    assert_eq!(type_str(unknown.kind), c"Unknown");
    // SAFETY: sets errno for the check below.
    unsafe { *__errno_location() = 0 };
    // SAFETY: v4's sockaddr is readable.
    assert!(unsafe { swr_v1_clients_address_string(AF_UNIX, v4.addr.as_ptr().cast()) }.is_null());
    assert_eq!(errno(), EAFNOSUPPORT);
}

/// `address.<name>`, in upstream's order.
pub(super) const CASES: super::Cases = &[
    ("basic", basic),
    ("dns_fail", dns_fail),
    ("path_to_long", path_to_long),
    ("bad_type", bad_type),
    ("type_str", type_str_),
    ("is_loopback_addr", is_loopback_addr),
    ("ipv4_addr", ipv4_addr),
    ("ipv6_addr", ipv6_addr),
    ("unix_addr", unix_addr),
    ("bad_addr", bad_addr),
    ("bad_port", bad_port),
    ("loopback_addr", loopback_addr),
    ("record", record),
];
