//! Upstream's `core-tests/src/network/dns.cpp`, one Rust function per TEST with the
//! same inputs and expected values. `dns_lookup` is crates/hooks' `resolve::dns_lookup`;
//! `get_ip_by_hosts`, the DNS server and hosts-path setters and `swoole_load_resolv_conf`
//! are crates/clients' `dns` functions; the name-resolver list
//! is Rust's (`swr_v1_core_name_resolver_*`), its `each` a remove; `GethostbynameRequest` and `GetaddrinfoRequest`
//! are crates/corebase's resolver record, so the cases drive that record
//! (`ResolverRequest`) as `network::gethostbyname`/`getaddrinfo` do; the
//! lookup itself is crates/hooks' `resolve::name_resolver_lookup`.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs;

use swoole_rs_clients::dns;
use swoole_rs_corebase::{
    NameResolverContext as Context, ResolverRequest, swr_v1_core_name_resolver_add,
    swr_v1_core_name_resolver_remove, swr_v1_core_resolver_free,
};

use super::{
    AF_INET, AF_INET6, EAI_NONAME, SOCK_STREAM, coroutine_cancel, current, go, last_error, run,
};

const SW_ERROR_DNSLOOKUP_RESOLVE_FAILED: c_int = 711;
const SW_ERROR_CO_CANCELED: c_int = 10017;
const SW_DNS_SERVER_PORT: c_int = 53;
const TYPE_USER: c_int = 2;
const TEST_HTTP_DOMAIN: &CStr = c"www.gov.cn";

/// `ctx = {type}`.
fn context_of(kind: c_int) -> Context {
    Context {
        kind,
        ..Context::default()
    }
}

unsafe extern "C-unwind" {
    fn swr_v1_clients_address_verify(family: c_int, host: *const c_char) -> bool;
}

/// `network::Address::verify_ip`.
pub(super) fn verify_ip(family: c_int, ip: &str) -> bool {
    let ip = CString::new(ip).unwrap();
    // SAFETY: a NUL-terminated address.
    unsafe { swr_v1_clients_address_verify(family, ip.as_ptr()) }
}

fn lookup(domain: &CStr, family: c_int, timeout: f64) -> Vec<String> {
    swoole_rs_hooks::resolve::dns_lookup(domain.to_bytes(), family, timeout)
        .into_iter()
        .map(|address| address.into_string().unwrap())
        .collect()
}

/// `get_ip_by_hosts`: its lines split by the classic locale's spaces, as
/// `std::istringstream` reads them.
fn hosts(domain: &CStr) -> String {
    let spaces: [u8; 256] =
        std::array::from_fn(|c| u8::from(b" \t\n\x0b\x0c\r".contains(&(c as u8))));
    let address = dns::hosts_lookup(domain.to_bytes(), &spaces).unwrap_or_default();
    String::from_utf8(address).unwrap()
}

fn set_dns_server(server: &str) {
    dns::set_dns_server(server.as_bytes());
}

/// `swoole_load_resolv_conf()`: `SwooleG.dns_resolvconf_path` is `SW_DNS_RESOLV_CONF`.
fn load_resolv_conf_() -> bool {
    dns::load_resolv_conf(dns::RESOLV_CONF)
}

/// `swoole_get_dns_server()`: the configured server, read from resolv.conf while none is.
fn dns_server() -> (String, c_int) {
    let read = || {
        let (host, port) = dns::server();
        let host = CStr::from_bytes_until_nul(&host).unwrap();
        (host.to_str().unwrap().to_owned(), port)
    };
    if read().0.is_empty() {
        load_resolv_conf_();
    }
    read()
}

/// `swoole_name_resolver_lookup(host_name, ctx)`.
fn name_resolver_lookup(host_name: &str, ctx: &mut Context) -> String {
    let mut out = Vec::new();
    swoole_rs_hooks::resolve::name_resolver_lookup(host_name.as_bytes(), ctx, &mut out);
    String::from_utf8(out).unwrap()
}

/// `GethostbynameRequest req(name, family); network::gethostbyname(&req)`: the status
/// and `req.addr`.
fn gethostbyname(name: &CStr, family: c_int) -> (c_int, String) {
    let req = ResolverRequest::create(family, 0, 0, name.to_bytes(), b"");
    assert!(!req.owner.is_null());
    // SAFETY: the record just made, live until it is freed below.
    let record = unsafe { &*req.owner };
    let status = record.host();
    let addr = record.address();
    // SAFETY: frees the record once.
    unsafe { swr_v1_core_resolver_free(req.owner) };
    (status, String::from_utf8(addr).unwrap())
}

struct Getaddrinfo {
    status: c_int,
    count: c_int,
    error: c_int,
    ip_list: Vec<String>,
}

/// `GetaddrinfoRequest req(hostname, family, SOCK_STREAM, 0, ""); network::getaddrinfo(&req);
/// req.parse_result(ip_list)`. `network::getaddrinfo`'s warning for an unknown family is a
/// log line; the lookup goes on.
fn getaddrinfo(hostname: &CStr, family: c_int) -> Getaddrinfo {
    let req = ResolverRequest::create(family, SOCK_STREAM, 0, hostname.to_bytes(), b"");
    assert!(!req.owner.is_null());
    // SAFETY: the record just made, live until it is freed below.
    let record = unsafe { &*req.owner };
    let status = record.lookup(|_| true);
    let ip_list = record
        .addresses()
        .into_iter()
        .map(|address| String::from_utf8(address).unwrap())
        .collect();
    let (count, error) = (record.fields().count, record.fields().error);
    // SAFETY: frees the record once.
    unsafe { swr_v1_core_resolver_free(req.owner) };
    Getaddrinfo {
        status,
        count,
        error,
        ip_list,
    }
}

fn lookup1() {
    run(|| assert!(!lookup(c"www.baidu.com", AF_INET, 10.0).is_empty()));
}

fn lookup_ipv6() {
    run(|| assert!(!lookup(c"www.google.com", AF_INET6, 2.0).is_empty()));
}

fn domain_not_found() {
    run(|| {
        assert_eq!(lookup(c"www.baidu.com-not-found", AF_INET, 2.0).len(), 0);
        assert_eq!(last_error(), SW_ERROR_DNSLOOKUP_RESOLVE_FAILED);
    });
}

fn bad_family() {
    run(|| assert!(!lookup(c"www.google.com", 9999, 2.0).is_empty()));
}

fn cancel() {
    run(|| {
        let co = current();
        go(move || {
            super::co_sleep(0.001);
            // SAFETY: the outer coroutine is suspended in its lookup.
            unsafe { coroutine_cancel(co) };
        });
        let list1 = lookup(c"www.baidu-not-found-for-cancel.com", AF_INET, 2.0);
        assert_eq!(list1.len(), 0);
        assert_eq!(last_error(), SW_ERROR_CO_CANCELED);
    });
}

fn gethostbyname_() {
    let (status, addr) = gethostbyname(TEST_HTTP_DOMAIN, AF_INET);
    assert_eq!(status, 0);
    assert!(verify_ip(AF_INET, &addr));

    let (status, addr) = gethostbyname(TEST_HTTP_DOMAIN, AF_INET6);
    assert_eq!(status, 0);
    assert!(verify_ip(AF_INET6, &addr));
}

fn getaddrinfo_() {
    let req = getaddrinfo(c"www.baidu.com", AF_INET);
    assert_eq!(req.status, 0);
    assert!(req.count > 0);
    for ip in &req.ip_list {
        assert!(verify_ip(AF_INET, ip));
    }
}

fn getaddrinfo_without_service() {
    // An empty service must be treated as no service at all; musl rejects an empty string with EAI_SERVICE.
    let req = getaddrinfo(c"localhost", AF_INET);
    assert_eq!(req.status, 0);
    assert!(req.count > 0);
    assert!(req.ip_list.iter().any(|ip| ip == "127.0.0.1"));
}

fn getaddrinfo_fail() {
    let req = getaddrinfo(c"www.baidu.com-not-exists", AF_INET);
    assert_eq!(req.status, -1);
    assert_eq!(req.error, EAI_NONAME);
}

fn getaddrinfo_ipv6() {
    let req = getaddrinfo(TEST_HTTP_DOMAIN, AF_INET6);
    assert_eq!(req.status, 0);
    assert!(req.count > 0);
    for ip in &req.ip_list {
        assert!(verify_ip(AF_INET6, ip));
    }
}

fn load_resolv_conf() {
    let port = super::socket::get_random_port();

    let ori_dns_server = dns_server();

    // with port
    set_dns_server(&format!("127.0.0.1:{port}")); // fake dns server
    let dns_server_ = dns_server();
    assert_eq!(dns_server_.0, "127.0.0.1");
    assert_eq!(dns_server_.1, port);

    // invalid port
    set_dns_server("127.0.0.1:808088");
    assert_eq!(dns_server().1, SW_DNS_SERVER_PORT);

    assert!(load_resolv_conf_());
    assert_eq!(dns_server(), ori_dns_server);
}

fn gethosts() {
    let hosts_file = c"/tmp/swoole_hosts";
    fs::write(
        hosts_file.to_str().unwrap(),
        "\n127.0.0.1\n127.0.0.1 localhost\n# 127.0.0.1 aaa.com\n       127.0.0.1 bbb.com               ccc.com      #ddd.com\n",
    )
    .unwrap();
    struct Unlink;
    impl Drop for Unlink {
        fn drop(&mut self) {
            let _ = fs::remove_file("/tmp/swoole_hosts");
        }
    }
    let _unlink = Unlink;

    dns::set_hosts_path(hosts_file.to_bytes());

    assert_eq!(hosts(c"localhost"), "127.0.0.1");
    assert_eq!(hosts(c"aaa.com"), "");
    assert_eq!(hosts(c"bbb.com"), "127.0.0.1");
    assert_eq!(hosts(c"ccc.com"), "127.0.0.1");
    assert_eq!(hosts(c"ddd.com"), "");
    assert_eq!(hosts(c"non.exist.com"), "");
}

fn name_resolver_test_fn_1() {
    let mut ctx = context_of(AF_INET);
    ctx.timeout = 1.0;
    assert_eq!("127.0.0.1", name_resolver_lookup("localhost", &mut ctx));
}

/// The user resolver: upstream's lambda, its answer in `out`; true when it
/// returned a non-empty string.
fn user_resolve(_: *mut c_void, domain: &[u8], ctx: &mut Context, out: &mut Vec<u8>) -> bool {
    if domain == b"name1" {
        *out = b"127.0.0.2".to_vec();
        return true;
    } else if domain == b"www.baidu.com" {
        ctx.is_final = true;
    }
    false
}

fn name_resolver_test_fn_2() {
    swr_v1_core_name_resolver_add(user_resolve, std::ptr::null_mut(), TYPE_USER, true);

    let mut ctx = context_of(AF_INET);
    assert_eq!("127.0.0.2", name_resolver_lookup("name1", &mut ctx));

    let mut ctx = context_of(AF_INET);
    assert_eq!("", name_resolver_lookup("www.baidu.com", &mut ctx));

    let mut ctx = context_of(AF_INET);
    assert_eq!("127.0.0.1", name_resolver_lookup("localhost", &mut ctx));

    // The traversal's body: remove the user resolver.
    swr_v1_core_name_resolver_remove(|_, kind| kind == TYPE_USER);

    let mut ctx = context_of(AF_INET);
    let ip = name_resolver_lookup("www.baidu.com", &mut ctx);
    assert!(verify_ip(AF_INET, &ip));
}

fn name_resolve_1() {
    name_resolver_test_fn_1();
    run(name_resolver_test_fn_1);
}

fn name_resolve_2() {
    name_resolver_test_fn_2();
    run(name_resolver_test_fn_2);
}

fn name_resolve_fail() {
    let mut ctx = context_of(AF_INET);
    let ip = name_resolver_lookup("www.baidu.com-not-exists", &mut ctx);
    assert!(ip.is_empty());
    assert_eq!(last_error(), SW_ERROR_DNSLOOKUP_RESOLVE_FAILED);
}

/// `dns.<name>`, in upstream's order.
pub(super) const CASES: super::Cases = &[
    ("lookup1", lookup1),
    ("lookup_ipv6", lookup_ipv6),
    ("domain_not_found", domain_not_found),
    ("bad_family", bad_family),
    ("cancel", cancel),
    ("gethostbyname", gethostbyname_),
    ("getaddrinfo", getaddrinfo_),
    ("getaddrinfo_without_service", getaddrinfo_without_service),
    ("getaddrinfo_fail", getaddrinfo_fail),
    ("getaddrinfo_ipv6", getaddrinfo_ipv6),
    ("load_resolv_conf", load_resolv_conf),
    ("gethosts", gethosts),
    ("name_resolve_1", name_resolve_1),
    ("name_resolve_2", name_resolve_2),
    ("name_resolve_fail", name_resolve_fail),
];
