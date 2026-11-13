//! Differential tests against the pinned upstream C++ (`swoole-rs-oracle`).

use std::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_clients::address::{
    Resolve, swr_v1_clients_address_assign, swr_v1_clients_address_loopback,
    swr_v1_clients_address_string, swr_v1_clients_address_verify,
};
use swoole_rs_clients::dns::{DnsExchange, INVALID_DOMAIN, hosts_lookup, set_hosts_path};
use swoole_rs_clients::ip::swr_v1_clients_ip_text;
use swoole_rs_clients::{
    URL_INVALID, URL_PORT_RANGE, cookie as split_cookie, domain_decode, domain_encode,
    swr_v1_clients_port, swr_v1_clients_url,
};
use swoole_rs_corebase::net_types::{AddressStorage, SocketAddress};
use swoole_rs_oracle::clients as upstream;

/// `php_swoole_http_parse_set_cookies`: the key, value offset and value length it
/// splits a `Set-Cookie` value into, which fix every Zend effect it makes (the
/// value it decodes, the key it stores under and the raw header it appends).
#[test]
fn set_cookie_split_matches_upstream() {
    let mut cookies: Vec<Vec<u8>> = [
        "",
        "a=b",
        "=b",
        "abc",
        "a=",
        "a=b;c=d",
        "a;b=c",
        "a=b=c",
        "a=%20+; path=/",
    ]
    .iter()
    .map(|cookie| cookie.as_bytes().to_vec())
    .collect();
    cookies.push(b"a\0x=b\0c;d".to_vec());
    for byte in 0..=255u8 {
        for prefix in [&b""[..], b"a=", b"a;", b"="] {
            cookies.push([prefix, &[byte], b"=x;y"].concat());
        }
    }
    for cookie in &cookies {
        let (mut key, mut offset, mut value) = (0, 0, 0);
        // SAFETY: the oracle reads `cookie.len()` bytes and writes three counts.
        unsafe {
            upstream::cookie(
                cookie.as_ptr().cast(),
                cookie.len(),
                &raw mut key,
                &raw mut offset,
                &raw mut value,
            );
        }
        let plan = split_cookie(cookie);
        assert_eq!(
            (plan.key_length, plan.value_offset, plan.value_length),
            (key, offset, value),
            "{cookie:?}"
        );
    }
    assert_eq!(cookies.len(), 1034);
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn set_errno(value: c_int) {
    #[cfg(target_os = "linux")]
    // SAFETY: this thread's errno slot.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: this thread's errno slot.
    let slot = unsafe { libc::__error() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
}

/// `bytes` with the NUL `std::string::c_str` puts after them.
fn c_str(bytes: &[u8]) -> Vec<u8> {
    [bytes, b"\0"].concat()
}

fn bytes(text: &[c_char]) -> Vec<u8> {
    text.iter().map(|&c| c.to_ne_bytes()[0]).collect()
}

/// The hosts the retired `ip_gate.py` checked: literals, every byte at every position of
/// three seeds, generated IPv4/IPv6 text, leading zeroes and Unix-path lengths.
fn ip_hosts() -> Vec<Vec<u8>> {
    let mut hosts: Vec<Vec<u8>> = [
        &b""[..],
        b"0.0.0.0",
        b"127.0.0.1",
        b"127.1.2.3",
        b"255.255.255.255",
        b"01.2.3.4",
        b"08.009.000.001",
        b"::ffff:01.02.03.004",
        b"1.2.3.256",
        b"1.2.3",
        b"+1.2.3.4",
        b"1.2.3.4 ",
        b"::",
        b"::1",
        b"::ffff:192.0.2.1",
        b"::192.0.2.1",
        b"2001:db8::1",
        b"1:2:3:4:5:6:7:8",
        b"::%lo0",
        b"::%",
        b"::1%tail",
        b"fe80::1%99999999999999999",
        b"1.2.3.4%scope",
        b"::1%scope:foo",
        b"::1%\xff",
        b"resolve-fail",
        b"localhost",
        b"127.0.0.1\0tail",
        b"::1\0tail",
    ]
    .iter()
    .map(|host| host.to_vec())
    .collect();
    for seed in [&b"127.0.0.1"[..], b"2001:db8::1", b"::ffff:192.0.2.1"] {
        for position in 0..seed.len() {
            for byte in 0..=255u8 {
                let mut host = seed.to_vec();
                if let Some(slot) = host.get_mut(position) {
                    *slot = byte;
                }
                hosts.push(host);
            }
        }
    }
    for n in 0..2048u32 {
        let octets = [n % 256, (n / 2) % 256, (n / 3) % 256, (n / 5) % 256];
        let v4 = octets.map(|octet| octet.to_string()).join(".");
        hosts.push(v4.into_bytes());
        hosts.push(format!("{n:x}:{:x}::{:x}", n * 31, n * 59).into_bytes());
    }
    for zeroes in 0..64 {
        let zeroes = "0".repeat(zeroes);
        hosts.push(format!("{zeroes}1.2.3.4").into_bytes());
        hosts.push(format!("::ffff:{zeroes}1.2.3.4").into_bytes());
    }
    for length in 103..=110 {
        hosts.push(vec![b'x'; length]);
    }
    hosts
}

/// `Address::verify_ip`: the result, errno and the 46-byte scratch it parses into.
#[test]
fn verify_ip_matches_upstream() {
    let seed = libc::in_addr { s_addr: 0 };
    // SAFETY: `seed` is an IPv4 address; the result is Rust's per-thread scratch.
    let scratch = unsafe { swr_v1_clients_address_string(libc::AF_INET, (&raw const seed).cast()) }
        .cast_mut();
    let mut cases = 0;
    for host in &ip_hosts() {
        let text = c_str(host);
        for family in [libc::AF_INET, libc::AF_INET6, libc::AF_UNIX, -1] {
            let (mut error, mut expected_scratch): (_, [c_char; 46]) = (0, [0; 46]);
            // SAFETY: `host` holds its bytes and the out parameters their sizes.
            let expected = unsafe {
                upstream::verify_ip(
                    family,
                    host.as_ptr().cast(),
                    host.len(),
                    &raw mut error,
                    expected_scratch.as_mut_ptr(),
                )
            };
            // SAFETY: `scratch` is the 46-byte buffer `verify_ip` parses into.
            unsafe { scratch.write_bytes(0x53, 46) };
            set_errno(17);
            // SAFETY: `text` is NUL-terminated.
            let actual = unsafe { swr_v1_clients_address_verify(family, text.as_ptr().cast()) };
            let actual_errno = errno();
            // SAFETY: as above.
            let parsed = unsafe { std::slice::from_raw_parts(scratch, 46) };
            assert_eq!(
                (actual, actual_errno, bytes(parsed)),
                (expected, error, bytes(&expected_scratch)),
                "{host:?} {family}"
            );
            cases += 1;
        }
    }
    assert_eq!(cases, 53_908, "every host in every family");
}

thread_local! {
    static RESOLVED: std::cell::RefCell<(c_int, Vec<u8>)> =
        const { std::cell::RefCell::new((0, Vec::new())) };
}

/// The oracle's `gethostbyname` double: counts the call, records the host, sets
/// errno 31, fails `resolve-fail` and fills the address with a family marker.
unsafe extern "C" fn resolve(family: c_int, host: *const c_char, out: *mut c_char) -> c_int {
    // SAFETY: the host is NUL-terminated.
    let host = unsafe { CStr::from_ptr(host) }.to_bytes().to_vec();
    let fail = host == b"resolve-fail";
    RESOLVED.with_borrow_mut(|resolved| *resolved = (resolved.0 + 1, host));
    set_errno(31);
    if fail {
        return -1;
    }
    let (marker, length) = if family == libc::AF_INET {
        (0x41, 4)
    } else {
        (0x61, 16)
    };
    // SAFETY: `out` holds the family's address.
    unsafe { out.cast::<u8>().write_bytes(marker, length) };
    0
}

/// What the hybrid `Address::assign(type, host, port, resolve)` leaves: it sets the
/// type, and the last error from Rust's code.
fn rust_assign(kind: c_int, host: &[u8], port: c_int, resolve_name: bool) -> upstream::Assign {
    // SAFETY: an all-zero record and trace are valid.
    let (mut address, mut trace): (SocketAddress, upstream::Assign) =
        unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    let size = size_of::<AddressStorage>();
    // SAFETY: `addr` holds `size` bytes.
    unsafe { (&raw mut address.addr).cast::<u8>().write_bytes(0x73, size) };
    address.len = 987;
    address.kind = kind;
    set_errno(17);
    RESOLVED.with_borrow_mut(|resolved| *resolved = (0, Vec::new()));
    let text = c_str(host);
    // SAFETY: the union, its length and a NUL-terminated host of `host.len()` bytes.
    let error = unsafe {
        swr_v1_clients_address_assign(
            &mut address.addr,
            &mut address.len,
            kind,
            text.as_ptr().cast(),
            host.len(),
            port,
            resolve_name.then_some::<Resolve>(resolve),
        )
    };
    trace.error = errno();
    let (dns_calls, dns_host) = RESOLVED.with_borrow(Clone::clone);
    trace.result = c_int::from(error == 0);
    trace.last_error = if error == 0 { 11 } else { error };
    trace.dns_calls = dns_calls;
    trace.dns_host_length = dns_host.len();
    for (slot, &byte) in trace.dns_host.iter_mut().zip(&dns_host) {
        *slot = byte;
    }
    trace.len = address.len;
    trace.kind = address.kind;
    trace.loopback = c_int::from(swr_v1_clients_address_loopback(&address));
    // SAFETY: `addr` holds `size` bytes.
    let stored =
        unsafe { std::slice::from_raw_parts((&raw const address.addr).cast::<u8>(), size) };
    for (slot, &byte) in trace.addr.iter_mut().zip(stored) {
        *slot = byte;
    }
    trace
}

fn assign_fields(trace: &upstream::Assign) -> (&[u8], u32, [c_int; 6], &[u8]) {
    (
        trace
            .addr
            .get(..size_of::<AddressStorage>())
            .unwrap_or_default(),
        trace.len,
        [
            trace.kind,
            trace.result,
            trace.error,
            trace.last_error,
            trace.dns_calls,
            trace.loopback,
        ],
        trace
            .dns_host
            .get(..trace.dns_host_length)
            .unwrap_or_default(),
    )
}

/// `Address::assign(type, host, port, resolve)`: the sockaddr union, length,
/// type, errno, last error, resolver calls and loopback it leaves.
#[test]
fn assign_matches_upstream() {
    // SAFETY: no arguments.
    let size = unsafe { upstream::address_size() };
    assert_eq!(size, size_of::<AddressStorage>(), "one sockaddr union");
    let hosts = ip_hosts();
    let mut cases = 0;
    for (index, host) in hosts.iter().enumerate() {
        if index >= 1024 && index < hosts.len() - 8 {
            continue;
        }
        for kind in 0..8 {
            for port in [-1, 0, 1, 65535, 65536] {
                for resolve_name in [false, true] {
                    // SAFETY: an all-zero trace is valid.
                    let mut expected: upstream::Assign = unsafe { std::mem::zeroed() };
                    // SAFETY: `host` holds its bytes and `expected` is a whole trace.
                    unsafe {
                        upstream::assign(
                            kind,
                            host.as_ptr().cast(),
                            host.len(),
                            port,
                            resolve_name,
                            &raw mut expected,
                        );
                    }
                    let actual = rust_assign(kind, host, port, resolve_name);
                    assert_eq!(
                        assign_fields(&actual),
                        assign_fields(&expected),
                        "{host:?} {kind} {port} {resolve_name}"
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(
        cases, 82_560,
        "1032 hosts, 8 types, 5 ports, both resolve modes"
    );
}

/// Both engines' `Address::is_loopback_addr` for every type over `bytes`.
fn check_loopback(bytes: &[u8; 16]) -> usize {
    for kind in 0..8 {
        // SAFETY: an all-zero record is valid.
        let mut address: SocketAddress = unsafe { std::mem::zeroed() };
        address.kind = kind;
        // SAFETY: the address field of a union member; nothing is read.
        let (field, length): (*mut u8, usize) = unsafe {
            if kind == 1 || kind == 2 {
                ((&raw mut address.addr.inet_v4.sin_addr).cast(), 4)
            } else {
                ((&raw mut address.addr.inet_v6.sin6_addr).cast(), 16)
            }
        };
        // SAFETY: the field holds `length` address bytes.
        unsafe { field.copy_from_nonoverlapping(bytes.as_ptr(), length) };
        // SAFETY: `bytes` holds 16 bytes.
        let expected = unsafe { upstream::loopback(kind, bytes.as_ptr()) };
        let actual = swr_v1_clients_address_loopback(&address);
        assert_eq!(actual, expected, "{bytes:?} {kind}");
    }
    8
}

/// `Address::is_loopback_addr` over every 16-bit pattern and every single set byte.
#[test]
fn loopback_matches_upstream() {
    let mut cases = 0;
    for n in 0..65536u32 {
        let pattern: [u8; 16] = std::array::from_fn(|i| (n >> (i % 2 * 8)).to_le_bytes()[0]);
        cases += check_loopback(&pattern);
    }
    for i in 0..16 {
        let mut pattern = [0; 16];
        pattern[i] = 1;
        cases += check_loopback(&pattern);
    }
    assert_eq!(cases, 524_416, "every pattern for every type");
}

/// The URLs the retired `gate.py` gate checked: edge cases, every protocol, host and port, and
/// every byte in the port, the host and a Unix path.
fn urls() -> Vec<Vec<u8>> {
    let mut urls: Vec<Vec<u8>> = [
        &b""[..],
        b"unix:///",
        b"unix:////",
        b"udg:///a",
        b"tcp://a:0",
        b"tcp://a:65536",
        b"tcp://a:999999999999999999999",
        b"tcp://[::1]:80",
        b"tcp://[]:80",
        b"tcp://a:01",
        b"tcp://a:+1",
        b"tcp://a:1\n",
        b"unix:///a?b",
        b"unix:///a#b",
        b"udp://]:80",
        b"tcp://a\0b:80",
    ]
    .iter()
    .map(|url| url.to_vec())
    .collect();
    let protocols = [
        &b"tcp://"[..],
        b"udp://",
        b"unix://",
        b"udg://",
        b"TCP://",
        b"",
    ];
    let hosts = [
        &b"a"[..],
        b"",
        b"[::1]",
        b"[a]",
        b"[",
        b"]",
        b"a:b",
        b"a[b",
        b"/",
        b"//",
        b"/a",
        b"/a?",
        b"/a#",
        b"a\nb",
        b"a b",
        b"a\0b",
    ];
    let ports = [
        &b""[..],
        b":",
        b":0",
        b":1",
        b":80",
        b":65535",
        b":65536",
        b":-1",
        b":+1",
        b":0002",
        b":1a",
        b":1:2",
        b":99999999999999999999999",
    ];
    for protocol in protocols {
        for host in hosts {
            for port in ports {
                urls.push([protocol, host, port].concat());
            }
        }
    }
    for byte in 0..=255u8 {
        urls.push([&b"tcp://a:"[..], &[byte]].concat());
        urls.push([&b"tcp://"[..], &[byte], b":80"].concat());
        urls.push([&b"unix:///"[..], &[byte]].concat());
    }
    urls
}

/// The hybrid `Address::assign(url)` over Rust's parse, into a recorder returning
/// `accept`: (result or -1 for `std::out_of_range`, type, host, port, error logs).
fn rust_url(url: &[u8], accept: bool) -> (c_int, c_int, Vec<u8>, c_int, c_int) {
    // SAFETY: `url` holds its bytes.
    let plan = unsafe { swr_v1_clients_url(url.as_ptr(), url.len()) };
    match plan.outcome {
        URL_PORT_RANGE => (-1, 87, Vec::new(), 0, 0),
        URL_INVALID => (0, 87, Vec::new(), 0, 1),
        _ => (
            c_int::from(accept),
            plan.kind,
            url.get(plan.host_offset..plan.host_offset + plan.host_length)
                .unwrap_or_default()
                .to_vec(),
            plan.port,
            0,
        ),
    }
}

/// `Address::assign(url)` and `Address::verify_port` against the pinned bodies.
#[test]
fn url_and_port_match_upstream() {
    let urls = urls();
    for url in &urls {
        for accept in [false, true] {
            let mut host: [c_char; 64] = [0; 64];
            let (mut kind, mut port, mut host_length, mut logs) = (0, 0, 0, 0);
            // SAFETY: `url` holds its bytes; `host` is longer than any URL here.
            let result = unsafe {
                upstream::url(
                    url.as_ptr().cast(),
                    url.len(),
                    accept,
                    &raw mut kind,
                    &raw mut port,
                    host.as_mut_ptr(),
                    &raw mut host_length,
                    &raw mut logs,
                )
            };
            assert_eq!(
                rust_url(url, accept),
                (result, kind, bytes(&host[..host_length]), port, logs),
                "{url:?} {accept}"
            );
        }
    }
    let mut cases = urls.len() * 2;
    for port in -3..=65538 {
        for connect in [false, true] {
            // SAFETY: no pointers.
            let expected = unsafe { upstream::verify_port(port, connect) };
            let actual = swr_v1_clients_port(port, u8::from(connect)) != 0;
            assert_eq!(actual, expected, "{port} {connect}");
            cases += 1;
        }
    }
    assert_eq!(cases, 135_148, "2032 URLs and 65542 ports, each both ways");
}

/// The hosts files the retired `gate.py` gate checked, each with the names it looked up.
fn hosts_files() -> Vec<(Vec<u8>, Vec<Vec<u8>>)> {
    let lines: [&[u8]; 13] = [
        b"",
        b"\n",
        b"127.0.0.1",
        b"127.0.0.1 localhost",
        b"# 127.0.0.1 aaa",
        b"       127.0.0.1 bbb               ccc      #ddd",
        b"a b#c d",
        b"a b#c d#e",
        b"a b b",
        b"a b\nz b",
        b"a b\r\nz b",
        b"\ra b",
        b"a b\0c",
    ];
    let names: [&[u8]; 12] = [
        b"a",
        b"b",
        b"localhost",
        b"bbb",
        b"ccc",
        b"ddd",
        b"d",
        b"d#e",
        b"b#c",
        b"b\0c",
        b"missing",
        b"",
    ];
    let mut files: Vec<(Vec<u8>, Vec<Vec<u8>>)> = lines
        .iter()
        .map(|line| {
            (
                line.to_vec(),
                names.iter().map(|name| name.to_vec()).collect(),
            )
        })
        .collect();
    for byte in 0..=255u8 {
        let token = vec![byte];
        files.push((
            [&b"addr "[..], &token, b" second\nother ", &token].concat(),
            vec![
                token,
                b"second".to_vec(),
                b"other".to_vec(),
                b"missing".to_vec(),
            ],
        ));
    }
    files
}

/// `get_ip_by_hosts` under the classic C++ locale, and one that also classes NUL,
/// 0xff and `@` as space; the hybrid passes Rust the space table the locale's
/// ctype reports.
#[test]
fn hosts_lookup_matches_upstream() {
    let path = std::env::temp_dir().join(format!("swoole-rs-hosts-{}", std::process::id()));
    let path_bytes = path.as_os_str().as_encoded_bytes();
    let path_c = c_str(path_bytes);
    set_hosts_path(path_bytes);
    let mut cases = 0;
    for extra_space in [false, true] {
        let mut spaces = [0u8; 256];
        // SAFETY: `spaces` holds 256 bytes.
        unsafe { upstream::spaces(extra_space, spaces.as_mut_ptr()) };
        for (file, names) in &hosts_files() {
            std::fs::write(&path, file).expect("hosts fixture");
            for name in names {
                let mut address: [c_char; 64] = [0; 64];
                // SAFETY: a NUL-terminated path; `name` and `address` hold their bytes.
                let length = unsafe {
                    upstream::hosts(
                        path_c.as_ptr().cast(),
                        name.as_ptr().cast(),
                        name.len(),
                        extra_space,
                        address.as_mut_ptr(),
                        address.len(),
                    )
                };
                let actual = hosts_lookup(name, &spaces).unwrap_or_default();
                assert_eq!(
                    actual,
                    bytes(&address[..length]),
                    "{file:?} {name:?} {extra_space}"
                );
                cases += 1;
            }
        }
    }
    std::fs::remove_file(&path).expect("hosts fixture");
    assert_eq!(
        cases, 2360,
        "13 files of 12 names and 256 of 4, in both locales"
    );
}

/// `SW_BUFFER_SIZE_STD`, the DNS packet, and the room after its 12-byte header.
const PACKET: usize = 8192;
const QUESTION: usize = PACKET - 12;

/// The bounds `dns::query` puts on the Rust encoder, over `n` bytes
/// of the NUL-terminated `source`.
fn rust_encode(source: &[u8], n: usize, dest: &mut [u8]) -> c_int {
    if n > QUESTION - 3 || source.get(n) == Some(&b'.') {
        return INVALID_DOMAIN;
    }
    domain_encode(
        source.get(..n).unwrap_or_default(),
        dest.get_mut(..QUESTION).unwrap_or_default(),
    )
}

fn upstream_encode(source: &[u8], n: usize, dest: &mut [u8]) -> c_int {
    // SAFETY: `source` is NUL-terminated after `n` bytes and `dest` holds `n + 3`.
    unsafe {
        upstream::domain_encode(
            source.as_ptr().cast(),
            c_int::try_from(n).unwrap_or(c_int::MAX),
            dest.as_mut_ptr().cast(),
        )
    }
}

fn signed_char() -> bool {
    c_char::MIN != 0
}

/// `domain_encode` over every byte at label lengths 1 to 256, odd dots and two
/// labels of 128 bytes split at every point, plus the trailing-dot rejection at a
/// shorter `n`.
#[test]
fn domain_encode_matches_upstream() {
    let mut domains: Vec<Vec<u8>> = [
        "",
        "a",
        "www.apple.com",
        "localhost",
        ".",
        "a.",
        ".a",
        "a..b",
        "a b.c",
        "a:b.c",
    ]
    .iter()
    .map(|domain| domain.as_bytes().to_vec())
    .collect();
    for byte in 0..=255u8 {
        for length in [1, 2, 4, 63, 127, 128, 255, 256] {
            domains.push(vec![byte; length]);
        }
    }
    for n in 1..=127 {
        domains.push([&vec![b'a'; n][..], b".", &vec![b'b'; 128 - n], b".example"].concat());
    }
    for domain in &domains {
        let source = c_str(domain);
        let (mut expected, mut actual) = (vec![b'!'; PACKET], vec![b'!'; PACKET]);
        assert_eq!(
            (rust_encode(&source, domain.len(), &mut actual), actual),
            (
                upstream_encode(&source, domain.len(), &mut expected),
                expected
            ),
            "{domain:?}"
        );
    }
    assert_eq!(domains.len(), 2185);
    let (mut expected, mut actual) = ([0u8; 32], [0u8; 32]);
    assert_eq!(
        (rust_encode(b"a.\0", 1, &mut actual), actual),
        (upstream_encode(b"a.\0", 1, &mut expected), expected)
    );
}

fn rust_decode(name: &mut [u8], capacity: usize) {
    domain_decode(name.get_mut(..capacity).unwrap_or_default(), signed_char());
}

fn upstream_decode(name: &mut [u8]) {
    // SAFETY: `name` is NUL-terminated within its bytes.
    unsafe { upstream::domain_decode(name.as_mut_ptr().cast()) }
}

/// `domain_decode` over three encoded labels of every length mix, then over rows
/// whose first label's length byte runs into an embedded NUL: the whole buffer,
/// padding included, must match.
#[test]
fn domain_decode_matches_upstream() {
    let labels = [
        "a".to_string(),
        "www".to_string(),
        "example".to_string(),
        "com".to_string(),
        "a".repeat(63),
        "z".repeat(127),
    ];
    let mut cases = 0;
    for a in &labels {
        for b in &labels {
            for c in &labels {
                let domain = c_str(format!("{a}.{b}.{c}").as_bytes());
                let mut expected = vec![b'!'; PACKET];
                upstream_encode(&domain, domain.len() - 1, &mut expected);
                let mut actual = expected.clone();
                let initialized = c_len(&expected) + 1;
                upstream_decode(&mut expected);
                rust_decode(&mut actual, initialized);
                assert_eq!(actual, expected, "{domain:?}");
                cases += 1;
            }
        }
    }
    let last = if signed_char() { 127 } else { 255 };
    for byte in 1..=last {
        for offset in [1, 2, 3] {
            let mut expected = vec![0u8; PACKET];
            expected[0] = byte;
            expected[1..=usize::from(byte)].fill(b'a');
            expected[usize::from(byte).min(offset)] = 0;
            let mut actual = expected.clone();
            upstream_decode(&mut expected);
            rust_decode(&mut actual, PACKET);
            assert_eq!(actual, expected, "{byte} {offset}");
            cases += 1;
        }
    }
    assert_eq!(cases, 216 + 3 * usize::from(last));
}

fn c_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())
}

/// One lookup's addresses, socket calls and last errors, and the query sent.
#[derive(Debug, Default, PartialEq)]
struct Lookup {
    addresses: Vec<Vec<u8>>,
    effects: Vec<Vec<u8>>,
    query: Vec<u8>,
}

unsafe extern "C" fn upstream_effect(
    context: *mut c_void,
    kind: c_int,
    text: *const c_char,
    length: usize,
) {
    // SAFETY: the context is the `Lookup` the oracle call borrows; `text` holds `length` bytes.
    let (lookup, text) = unsafe {
        (
            &mut *context.cast::<Lookup>(),
            std::slice::from_raw_parts(text.cast::<u8>(), length).to_vec(),
        )
    };
    match kind {
        0 => lookup.addresses.push(text),
        1 => lookup.effects.push(text),
        _ => lookup.query = text,
    }
}

/// The socket double the Rust lookup runs over: the fixture, its errCode and the lookup.
struct Exchange {
    record_type: c_int,
    receive_mode: c_int,
    send_success: bool,
    canceled: bool,
    bad_id: bool,
    lookup: Lookup,
}

impl DnsExchange for Exchange {
    fn send(&mut self, data: &[u8]) -> bool {
        self.lookup.effects.push(b"send".to_vec());
        self.lookup.query = data.to_vec();
        self.send_success
    }

    /// The patched caller's `recv`: a failed receive reports `-ECANCELED` when the
    /// socket's errCode is ECANCELED.
    fn recv(&mut self, data: &mut [u8]) -> isize {
        self.lookup.effects.push(b"receive".to_vec());
        // SAFETY: the query holds its bytes and `data` its length.
        let n = unsafe {
            upstream::dns_answer(
                self.record_type,
                self.receive_mode,
                self.bad_id,
                self.lookup.query.as_ptr().cast(),
                self.lookup.query.len(),
                data.as_mut_ptr().cast(),
                data.len(),
            )
        };
        if n <= 0 && self.canceled {
            -isize::try_from(libc::ECANCELED).unwrap_or(0)
        } else {
            n
        }
    }

    fn push(&mut self, text: &[u8]) {
        self.lookup.addresses.push(text.to_vec());
    }
}

/// `dns_lookup_impl_with_socket` over a UDP socket double, for A, AAAA and CNAME
/// answers, with and without a timeout, a failed send, a failed, short or whole
/// receive, cancellation and a mismatched id. The Rust side is the patched
/// caller: its socket calls, `dns::query` and the last error it sets.
#[test]
fn dns_lookup_matches_upstream() {
    let mut request_id = 1u16;
    let mut cases = 0;
    for domain in [c"a.co", c"abc.test", c"long.example"] {
        for record_type in [1, 28, 5] {
            let family = if record_type == 28 {
                libc::AF_INET6
            } else {
                libc::AF_INET
            };
            for timeout in [0.0, 0.1] {
                for send_success in [false, true] {
                    for receive_mode in [0, 1, 2] {
                        for canceled in [false, true] {
                            for bad_id in [false, true] {
                                let mut expected = Lookup::default();
                                // SAFETY: `domain` is NUL-terminated and `expected` outlives the call.
                                unsafe {
                                    upstream::dns_lookup(
                                        domain.as_ptr(),
                                        family,
                                        timeout,
                                        record_type,
                                        receive_mode,
                                        send_success,
                                        canceled,
                                        bad_id,
                                        (&raw mut expected).cast(),
                                        upstream_effect,
                                    );
                                }
                                let mut exchange = Exchange {
                                    record_type,
                                    receive_mode,
                                    send_success,
                                    canceled,
                                    bad_id,
                                    lookup: Lookup::default(),
                                };
                                if timeout > 0.0 {
                                    exchange.lookup.effects.push(b"timeout".to_vec());
                                }
                                let error = swoole_rs_clients::dns::query(
                                    domain,
                                    family,
                                    request_id,
                                    &mut exchange,
                                    signed_char(),
                                );
                                request_id = request_id.wrapping_add(1);
                                if error != 0 && error != INVALID_DOMAIN {
                                    exchange
                                        .lookup
                                        .effects
                                        .push(format!("error:{error}").into_bytes());
                                }
                                assert_eq!(
                                    exchange.lookup, expected,
                                    "{domain:?} {record_type} {timeout} {send_success} {receive_mode} {canceled} {bad_id}"
                                );
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 432);
}

/// `parse_ip_address` against `swr_v1_clients_ip_text`, which the DNS and c-ares
/// lookups call, for both families over every two-byte pattern, every byte at
/// every position over zeros and ones, and 65,536 pseudo-random addresses: the
/// text, errno and the input bytes after.
#[test]
fn ip_text_matches_upstream() {
    let mut cases = 0;
    let mut compare = |address: &mut [u8; 32]| {
        for (family, version) in [(libc::AF_INET, 4), (libc::AF_INET6, 6)] {
            let saved = *address;
            let mut text: [c_char; 64] = [0; 64];
            set_errno(17);
            // SAFETY: `address` holds 32 bytes and `text` 64.
            let length = unsafe {
                upstream::ip_text(
                    address.as_mut_ptr().cast(),
                    family,
                    text.as_mut_ptr(),
                    text.len(),
                )
            };
            let expected = (bytes(&text[..length]), errno(), *address);
            set_errno(17);
            let actual = swr_v1_clients_ip_text(version, &address[..]);
            let length = usize::try_from(actual.length).unwrap_or(0);
            assert_eq!(
                (actual.bytes[..length].to_vec(), errno(), *address),
                expected,
                "{saved:?} {family}"
            );
            cases += 1;
        }
    };
    let mut bytes = [0u8; 32];
    for n in 0..=u16::MAX {
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = n.to_le_bytes()[index % 2];
        }
        compare(&mut bytes);
    }
    for position in 0..16 {
        for n in 0..=255 {
            for fill in [0, 255] {
                bytes.fill(fill);
                bytes[position] = n;
                compare(&mut bytes);
            }
        }
    }
    let mut random = 1_234_567u32;
    for _ in 0..65536 {
        for byte in &mut bytes {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = random.to_be_bytes()[0];
        }
        compare(&mut bytes);
    }
    assert_eq!(cases, 2 * (65536 + 16 * 256 * 2 + 65536));
}
