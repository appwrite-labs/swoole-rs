//! `dns_lookup_impl_with_socket`: one A or AAAA query to the configured server over
//! UDP and the addresses in its answer. Native code passes the coroutine socket's
//! `sendto` and `recv` and collects the addresses. Every read of the reply stays
//! inside the packet buffer.
//!
//! The resolver's configuration (`SwooleG.dns_server` and `dns_hosts_path`
//! upstream) is [`CONFIG`]: the DNS server, set or read from `resolv.conf`, and the
//! hosts file.
use crate::ip::swr_v1_clients_ip_text;
use crate::{
    swr_v1_clients_dns_endpoint, swr_v1_clients_domain_decode, swr_v1_clients_domain_encode,
    swr_v1_clients_hosts_match, swr_v1_clients_port, swr_v1_clients_resolv_line,
};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use swoole_rs_corebase::log;
use swoole_rs_error::ErrorCode;

/// `SW_BUFFER_SIZE_STD`.
const PACKET: usize = 8192;
/// `sizeof(RecordHeader)`.
const HEADER: usize = 12;
/// The fixed part of a resource record after its (compressed) name.
const RECORD: usize = 10;
/// Answers read, and bytes kept per answer's data.
const MAX_ANSWERS: usize = 10;
const RDATA: usize = 254;
const A: u16 = 0x01;
const AAAA: u16 = 0x1c;

/// `SW_ERROR_DNSLOOKUP_RESOLVE_FAILED` and `SW_ERROR_CO_CANCELED`.
const RESOLVE_FAILED: c_int = 711;
const CANCELED: c_int = 10017;
/// The domain does not encode: native warns `invalid domain[...]`.
pub const INVALID_DOMAIN: c_int = -1;

/// Sends `length` bytes to the DNS server; false on failure.
pub type Send = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool;
/// Receives into `capacity` bytes; the length, 0/-1 on failure, `-ECANCELED` if
/// the coroutine was cancelled.
pub type Recv = unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> isize;
/// Collects one address in text form.
pub type Push = unsafe extern "C" fn(*mut c_void, *const u8, usize);

fn u16_at(packet: &[u8], at: usize) -> u16 {
    let byte = |offset: usize| packet.get(offset).copied().unwrap_or(0);
    u16::from_be_bytes([byte(at), byte(at + 1)])
}

/// Copies `bytes` into `buffer` at `at`, as far as `buffer` reaches.
fn put(buffer: &mut [u8], at: usize, bytes: &[u8]) {
    let slots = buffer.iter_mut().skip(at);
    for (slot, &byte) in slots.zip(bytes) {
        *slot = byte;
    }
}

fn c_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())
}

/// `SW_DNS_SERVER_PORT`.
const DNS_PORT: c_int = 53;
/// `SW_PATH_HOSTS`.
const HOSTS: &[u8] = b"/etc/hosts";
/// `SW_DNS_RESOLV_CONF`, which upstream's `SwooleG.dns_resolvconf_path` always held.
pub const RESOLV_CONF: &CStr = c"/etc/resolv.conf";

/// The DNS server (an empty host until set or read from `resolv.conf`) and the
/// hosts file (empty: [`HOSTS`]).
struct Config {
    host: Vec<u8>,
    port: c_int,
    hosts: Vec<u8>,
}

static CONFIG: Mutex<Config> = Mutex::new(Config {
    host: Vec::new(),
    port: 0,
    hosts: Vec::new(),
});

fn config() -> MutexGuard<'static, Config> {
    CONFIG.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `swoole_set_dns_server`: `host[:port]`, where an absent or invalid port is 53.
/// The text ends at its first NUL, and the port converts as `sw_atoi` (`strtol`
/// narrowed to `int`).
fn set_dns_server(server: &[u8]) {
    // SAFETY: an initialized span.
    let plan = unsafe { swr_v1_clients_dns_endpoint(server.as_ptr(), server.len()) };
    let mut port = DNS_PORT;
    if plan.has_port != 0 {
        let digits = server.get(plan.port_offset..).unwrap_or_default();
        let mut text = crate::c_str(digits).to_vec();
        text.push(0);
        // SAFETY: NUL-terminated.
        let parsed = unsafe { libc::strtol(text.as_ptr().cast(), std::ptr::null_mut(), 10) };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "sw_atoi narrows strtol's long to int"
        )]
        let parsed = parsed as c_int;
        if swr_v1_clients_port(parsed, 1) != 0 {
            port = parsed;
        }
    }
    let mut config = config();
    config.host = server.get(..plan.host_length).unwrap_or_default().to_vec();
    config.port = port;
}

/// `swoole_set_dns_server(server)`.
/// # Safety
/// `server` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_set_dns_server(server: *const u8, length: usize) {
    // SAFETY: per the caller.
    set_dns_server(unsafe { std::slice::from_raw_parts(server, length) });
}

/// The DNS server: its host through `push` (with `context`), its port returned.
/// # Safety
/// `push` is native's, valid with `context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_dns_server(context: *mut c_void, push: Push) -> c_int {
    let (host, port) = {
        let config = config();
        (config.host.clone(), config.port)
    };
    // SAFETY: per the caller; the native push copies `host`.
    unsafe { push(context, host.as_ptr(), host.len()) };
    port
}

/// `swoole_load_resolv_conf`'s read: the first `nameserver` of the file at `path`
/// becomes the DNS server. 1 found, 0 none, -1 the file did not open (warned with
/// fopen's errno).
/// # Safety
/// `path` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_load_resolv_conf(path: *const c_char) -> c_int {
    let mut server = [0 as c_char; 16];
    // SAFETY: per the caller; `server` holds 16 bytes.
    let found = unsafe { resolv_conf(path, server.as_mut_ptr()) };
    if found < 0 {
        // SAFETY: per the caller.
        let path = unsafe { CStr::from_ptr(path) }.to_string_lossy();
        log::sys_warning(
            c"swoole_load_resolv_conf",
            format_args!("fopen({path}) failed"),
        );
    }
    if found > 0 {
        // SAFETY: NUL-terminated by the read.
        set_dns_server(unsafe { CStr::from_ptr(server.as_ptr()) }.to_bytes());
    }
    found
}

/// `swoole_set_hosts_path(path)`.
/// # Safety
/// `path` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_set_hosts_path(path: *const u8, length: usize) {
    // SAFETY: per the caller.
    let path = unsafe { std::slice::from_raw_parts(path, length) }.to_vec();
    config().hosts = path;
}

/// Queries `domain` for `family`'s address records with `request_id`, pushing each
/// address of the answer; 0, [`INVALID_DOMAIN`], or the code native sets as the
/// last error (also when the answer has no address).
///
/// # Safety
/// `domain` is NUL-terminated; `send`, `recv` and `push` are native's, valid with
/// `context`. `signed_char` is native `char`'s signedness.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_dns_query(
    domain: *const c_char,
    family: c_int,
    request_id: u16,
    context: *mut c_void,
    send: Send,
    recv: Recv,
    push: Push,
    signed_char: bool,
) -> c_int {
    let mut packet = vec![0u8; PACKET];
    let [id_high, id_low] = request_id.to_be_bytes();
    // ID, RD, one question.
    put(&mut packet, 0, &[id_high, id_low, 1, 0, 0, 1]);

    // SAFETY: the caller passes a NUL-terminated domain.
    let name = unsafe { CStr::from_ptr(domain) }.to_bytes();
    let capacity = PACKET - HEADER;
    if name.len() > capacity - 3 {
        return INVALID_DOMAIN;
    }
    let question = packet.get_mut(HEADER..).unwrap_or_default();
    // SAFETY: `name` and `question` are separate buffers, `question` at least
    // `capacity` bytes long.
    if unsafe {
        swr_v1_clients_domain_encode(name.as_ptr(), name.len(), question.as_mut_ptr(), capacity)
    } < 0
    {
        return INVALID_DOMAIN;
    }
    let mut steps = HEADER + c_len(question) + 1;
    let [type_high, type_low] = if family == libc::AF_INET6 { AAAA } else { A }.to_be_bytes();
    // QTYPE, QCLASS IN.
    put(&mut packet, steps, &[type_high, type_low, 0, 1]);
    steps += 4;
    let query_initialized = PACKET.min(steps.max(HEADER + name.len() + 3));

    // SAFETY: the native send takes `steps` bytes of the packet.
    if !unsafe { send(context, packet.as_ptr(), steps) } {
        return RESOLVE_FAILED;
    }
    // SAFETY: the native recv writes at most `PACKET - 1` bytes into the packet.
    let received = unsafe { recv(context, packet.as_mut_ptr(), PACKET - 1) };
    let Ok(received @ 1..) = usize::try_from(received) else {
        return if received == -isize::try_from(libc::ECANCELED).unwrap_or(0) {
            CANCELED
        } else {
            RESOLVE_FAILED
        };
    };
    let received = received.min(PACKET - 1);
    put(&mut packet, received, &[0]);
    let initialized = query_initialized.max(received + 1);

    let question = packet.get_mut(HEADER..initialized).unwrap_or_default();
    // SAFETY: `question` is initialized and writable for its length.
    unsafe {
        swr_v1_clients_domain_decode(question.as_mut_ptr(), question.len(), u8::from(signed_char))
    };
    let mut steps = HEADER + c_len(packet.get(HEADER..).unwrap_or_default()) + 2 + 4;

    let answers = usize::from(u16_at(&packet, 6)).min(MAX_ANSWERS);
    let mut records = Vec::with_capacity(answers);
    for _ in 0..answers {
        // The answer's name is a two-byte compression pointer.
        steps += 2;
        let kind = u16_at(&packet, steps);
        let length = usize::from(u16_at(&packet, steps + 8));
        steps += RECORD;
        let mut data = [0u8; RDATA];
        let source = packet.get(steps..).unwrap_or_default();
        for (slot, &byte) in data.iter_mut().zip(source).take(length) {
            *slot = byte;
        }
        records.push((kind, data));
        steps = steps.saturating_add(length);
    }

    if u16_at(&packet, 0) != request_id {
        return RESOLVE_FAILED;
    }
    let mut found = false;
    for (kind, data) in &records {
        let version = match *kind {
            A => 4,
            AAAA => 6,
            _ => continue,
        };
        // SAFETY: `data` holds the 4- or 16-byte address.
        let text = unsafe { swr_v1_clients_ip_text(version, data.as_ptr()) };
        let length = usize::try_from(text.length).unwrap_or(0);
        // SAFETY: the native push copies `length` bytes.
        unsafe { push(context, text.bytes.as_ptr(), length) };
        found = true;
    }
    if found { 0 } else { RESOLVE_FAILED }
}

/// The coroutine UDP socket `dns_lookup_impl_with_socket` exchanges its query over
/// (`new Socket(SW_SOCK_UDP)`; dropping it deletes it).
pub trait DnsSocket {
    /// `sendto(host, port, data)`; false on failure.
    fn sendto(&mut self, host: &CStr, port: c_int, data: &[u8]) -> bool;
    /// `recv` into `buffer`, as [`Recv`].
    fn recv(&mut self, buffer: &mut [u8]) -> isize;
}

struct Exchange<S> {
    socket: S,
    host: Vec<u8>,
    port: c_int,
    found: Vec<Vec<u8>>,
}

unsafe extern "C" fn exchange_send<S: DnsSocket>(
    context: *mut c_void,
    data: *const u8,
    length: usize,
) -> bool {
    // SAFETY: `lookup`'s live exchange.
    let exchange = unsafe { &mut *context.cast::<Exchange<S>>() };
    // SAFETY: the query's `length` bytes.
    let data = unsafe { std::slice::from_raw_parts(data, length) };
    let host = CStr::from_bytes_until_nul(&exchange.host).unwrap_or_default();
    exchange.socket.sendto(host, exchange.port, data)
}

unsafe extern "C" fn exchange_recv<S: DnsSocket>(
    context: *mut c_void,
    data: *mut u8,
    capacity: usize,
) -> isize {
    // SAFETY: `lookup`'s live exchange.
    let exchange = unsafe { &mut *context.cast::<Exchange<S>>() };
    // SAFETY: the packet buffer's `capacity` writable bytes.
    exchange
        .socket
        .recv(unsafe { std::slice::from_raw_parts_mut(data, capacity) })
}

unsafe extern "C" fn exchange_push<S: DnsSocket>(
    context: *mut c_void,
    text: *const u8,
    length: usize,
) {
    // SAFETY: `lookup`'s live exchange.
    let exchange = unsafe { &mut *context.cast::<Exchange<S>>() };
    // SAFETY: the address's `length` bytes.
    exchange
        .found
        .push(unsafe { std::slice::from_raw_parts(text, length) }.to_vec());
}

/// The DNS server's host, NUL-terminated, and port.
pub(crate) fn server() -> (Vec<u8>, c_int) {
    let config = config();
    let mut host = config.host.clone();
    host.push(0);
    (host, config.port)
}

/// The next query's id (upstream's `static uint16_t dns_request_id`).
static REQUEST_ID: AtomicU16 = AtomicU16::new(1);

/// `dns_lookup_impl_with_socket(domain, family)`: one query to the DNS server, read
/// from [`RESOLV_CONF`] while none is set, over the socket `open` makes; the answer's
/// addresses. A failure is the last error, or a warning for a domain that doesn't
/// encode.
pub fn lookup<S: DnsSocket>(
    domain: &CStr,
    family: c_int,
    open: impl FnOnce() -> S,
) -> Vec<Vec<u8>> {
    let (mut host, mut port) = server();
    if host.len() == 1 {
        // SAFETY: a NUL-terminated path.
        if unsafe { swr_v1_clients_load_resolv_conf(RESOLV_CONF.as_ptr()) } <= 0 {
            log::set_last_error(ErrorCode::SW_ERROR_DNSLOOKUP_NO_SERVER as c_int);
            return Vec::new();
        }
        (host, port) = server();
    }
    let mut exchange = Exchange {
        socket: open(),
        host,
        port,
        found: Vec::new(),
    };
    // SAFETY: a NUL-terminated domain; the trampolines borrow `exchange` for the call.
    let error = unsafe {
        swr_v1_clients_dns_query(
            domain.as_ptr(),
            family,
            REQUEST_ID.fetch_add(1, Ordering::Relaxed),
            (&raw mut exchange).cast(),
            exchange_send::<S>,
            exchange_recv::<S>,
            exchange_push::<S>,
            c_char::MIN != 0,
        )
    };
    if error == INVALID_DOMAIN {
        let domain = domain.to_string_lossy();
        log::warning(
            c"dns_lookup_impl_with_socket",
            format_args!("invalid domain[{domain}]"),
        );
    } else if error != 0 {
        log::set_last_error(error);
    }
    exchange.found
}

pub use swoole_rs_corebase::{Resolved, swr_v1_clients_getaddrinfo, swr_v1_clients_gethostbyname};

/// The first `nameserver` of the file at `path`, read in `fgets` lines of up to 99
/// bytes, into `server` (16 bytes, NUL-terminated); 1 found, 0 none, -1 the file
/// did not open (errno is fopen's).
///
/// # Safety
/// `path` is NUL-terminated; `server` holds 16 bytes.
unsafe fn resolv_conf(path: *const c_char, server: *mut c_char) -> c_int {
    // SAFETY: both strings are NUL-terminated.
    let file = unsafe { libc::fopen(path, c"rt".as_ptr()) };
    if file.is_null() {
        return -1;
    }
    let mut line = [0 as c_char; 100];
    let mut found = 0;
    // SAFETY: `line` holds 100 bytes and `file` is open.
    while !unsafe { libc::fgets(line.as_mut_ptr(), 100, file) }.is_null() {
        // SAFETY: fgets NUL-terminated the line.
        let text = unsafe { CStr::from_ptr(line.as_ptr()) }.to_bytes();
        // SAFETY: `text` is the line's bytes.
        let plan = unsafe { swr_v1_clients_resolv_line(text.as_ptr(), text.len()) };
        if plan.kind == 0 {
            continue;
        }
        if plan.kind == 1 && plan.length < 16 {
            // SAFETY: the caller's `server` holds 16 bytes.
            let server = unsafe { std::slice::from_raw_parts_mut(server.cast::<u8>(), 16) };
            let token = text
                .get(plan.offset..plan.offset + plan.length)
                .unwrap_or_default();
            put(server, 0, token);
            put(server, token.len(), &[0]);
            found = c_int::from(plan.length != 0);
        }
        break;
    }
    // SAFETY: `file` came from fopen.
    unsafe { libc::fclose(file) };
    found
}

/// `get_ip_by_hosts`: the address `domain` has in the hosts file, pushed as text;
/// whether one was found. Lines split as `std::getline` does, cut at the first
/// `#`; `spaces` is the native locale's 256-entry space table.
///
/// # Safety
/// `domain` holds `domain_length` bytes, `spaces` 256; `push` is native's, valid
/// with `context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_hosts_lookup(
    domain: *const u8,
    domain_length: usize,
    spaces: *const u8,
    context: *mut c_void,
    push: Push,
) -> bool {
    let mut path = config().hosts.clone();
    if path.is_empty() {
        path = HOSTS.to_vec();
    }
    // `std::ifstream` opens the path up to its first NUL.
    path.truncate(c_len(&path));
    let Ok(contents) =
        std::fs::read(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&path))
    else {
        return false;
    };
    let mut lines: Vec<&[u8]> = contents.split(|&byte| byte == b'\n').collect();
    if contents.last() == Some(&b'\n') {
        lines.pop();
    }
    for line in lines {
        let mut line = line.to_vec();
        if let Some(comment) = line.iter_mut().find(|byte| **byte == b'#') {
            *comment = 0;
        }
        // SAFETY: `line` and `domain` are initialized; `spaces` holds 256 entries.
        let found = unsafe {
            swr_v1_clients_hosts_match(line.as_ptr(), line.len(), domain, domain_length, spaces)
        };
        if found.found != 0 {
            let address = line
                .get(found.offset..found.offset + found.length)
                .unwrap_or_default();
            // SAFETY: the native push copies `address`.
            unsafe { push(context, address.as_ptr(), address.len()) };
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Exchange {
        query: Vec<u8>,
        reply: Vec<u8>,
        results: Vec<String>,
    }

    unsafe extern "C" fn sent(context: *mut c_void, data: *const u8, length: usize) -> bool {
        let exchange = unsafe { &mut *context.cast::<RefCell<Exchange>>() };
        exchange.borrow_mut().query = unsafe { std::slice::from_raw_parts(data, length) }.to_vec();
        true
    }
    unsafe extern "C" fn answered(context: *mut c_void, out: *mut u8, capacity: usize) -> isize {
        let exchange = unsafe { &*context.cast::<RefCell<Exchange>>() };
        let mut exchange = exchange.borrow_mut();
        let id = [exchange.query[0], exchange.query[1]];
        exchange.reply[..2].copy_from_slice(&id);
        let n = exchange.reply.len().min(capacity);
        unsafe { std::ptr::copy_nonoverlapping(exchange.reply.as_ptr(), out, n) };
        isize::try_from(n).unwrap()
    }
    unsafe extern "C" fn cancelled(_: *mut c_void, _: *mut u8, _: usize) -> isize {
        -isize::try_from(libc::ECANCELED).unwrap()
    }
    unsafe extern "C" fn collect(context: *mut c_void, text: *const u8, length: usize) {
        let exchange = unsafe { &*context.cast::<RefCell<Exchange>>() };
        let text = unsafe { std::slice::from_raw_parts(text, length) };
        exchange
            .borrow_mut()
            .results
            .push(String::from_utf8(text.to_vec()).unwrap());
    }

    fn reply(records: &[(u16, &[u8])]) -> Vec<u8> {
        let mut packet = vec![
            0,
            0,
            0x81,
            0x80,
            0,
            1,
            0,
            u8::try_from(records.len()).unwrap(),
            0,
            0,
            0,
            0,
        ];
        packet.extend_from_slice(b"\x07example\x04test\x00\x00\x01\x00\x01");
        for (kind, data) in records {
            packet.extend_from_slice(&[0xc0, 12]);
            packet.extend_from_slice(&kind.to_be_bytes());
            packet.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
            packet.extend_from_slice(&u16::try_from(data.len()).unwrap().to_be_bytes());
            packet.extend_from_slice(data);
        }
        packet
    }

    fn query(family: c_int, reply: Vec<u8>, recv: Recv) -> (c_int, Exchange) {
        let exchange = RefCell::new(Exchange {
            query: Vec::new(),
            reply,
            results: Vec::new(),
        });
        let context = (&raw const exchange).cast_mut().cast();
        let error = unsafe {
            swr_v1_clients_dns_query(
                c"example.test".as_ptr(),
                family,
                7,
                context,
                sent,
                recv,
                collect,
                true,
            )
        };
        (error, exchange.into_inner())
    }

    #[test]
    fn asks_once_and_reads_addresses() {
        let cname: &[u8] = b"\x03www\xc0\x0c";
        let (error, exchange) = query(
            libc::AF_INET,
            reply(&[(5, cname), (A, &[192, 0, 2, 1]), (A, &[192, 0, 2, 2])]),
            answered,
        );
        assert_eq!(error, 0);
        assert_eq!(exchange.results, ["192.0.2.1", "192.0.2.2"]);
        assert_eq!(&exchange.query[..12], &[0, 7, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            &exchange.query[12..],
            b"\x07example\x04test\x00\x00\x01\x00\x01"
        );

        let six = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let (error, exchange) = query(libc::AF_INET6, reply(&[(AAAA, &six)]), answered);
        assert_eq!(
            (error, exchange.results),
            (
                0,
                vec!["2001:0db8:0000:0000:0000:0000:0000:0001".to_string()]
            )
        );
        assert_eq!(
            &exchange.query[exchange.query.len() - 4..],
            &[0, 0x1c, 0, 1]
        );

        assert_eq!(
            query(libc::AF_INET, reply(&[(5, cname)]), answered).0,
            RESOLVE_FAILED
        );
        assert_eq!(query(libc::AF_INET, reply(&[]), cancelled).0, CANCELED);
        let mut truncated = reply(&[(A, &[192, 0, 2, 1])]);
        truncated[7] = 200;
        assert_eq!(
            query(libc::AF_INET, truncated, answered).1.results,
            ["192.0.2.1"]
        );
    }

    unsafe extern "C" fn ignore(_: c_int) {}

    #[test]
    fn resolves_through_libc() {
        let mut addr = [0 as c_char; 16];
        assert_eq!(
            unsafe {
                swr_v1_clients_gethostbyname(
                    libc::AF_INET,
                    c"localhost".as_ptr(),
                    addr.as_mut_ptr(),
                )
            },
            0
        );
        assert_eq!(
            addr[..4]
                .iter()
                .map(|&b| b.to_ne_bytes()[0])
                .collect::<Vec<_>>(),
            [127, 0, 0, 1]
        );
        assert_eq!(
            unsafe {
                swr_v1_clients_gethostbyname(
                    libc::AF_UNIX,
                    c"localhost".as_ptr(),
                    addr.as_mut_ptr(),
                )
            },
            -1
        );

        let mut results: [libc::sockaddr_in6; 16] = unsafe { std::mem::zeroed() };
        let found = unsafe {
            swr_v1_clients_getaddrinfo(
                c"127.0.0.1".as_ptr(),
                c"80".as_ptr(),
                libc::AF_INET,
                libc::SOCK_STREAM,
                0,
                results.as_mut_ptr(),
                16,
                ignore,
            )
        };
        assert_eq!(found, Resolved { error: 0, count: 1 });
        let v4 = unsafe { &*results.as_ptr().cast::<libc::sockaddr_in>() };
        assert_eq!(
            (u16::from_be(v4.sin_port), v4.sin_addr.s_addr.to_ne_bytes()),
            (80, [127, 0, 0, 1])
        );
        let failed = unsafe {
            swr_v1_clients_getaddrinfo(
                c"127.0.0.1".as_ptr(),
                c"no-such-service-swr".as_ptr(),
                libc::AF_INET,
                0,
                0,
                results.as_mut_ptr(),
                16,
                ignore,
            )
        };
        assert_ne!(failed.error, 0);
    }

    #[test]
    fn reads_resolv_conf_and_hosts() {
        let dir = std::env::temp_dir().join(format!("swr-dns-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let resolv = dir.join("resolv.conf");
        std::fs::write(
            &resolv,
            "# comment\nsearch lan\nnameserver 10.0.0.53\nnameserver 10.0.0.54\n",
        )
        .unwrap();
        let path = std::ffi::CString::new(resolv.to_str().unwrap()).unwrap();
        let server = || {
            let exchange = RefCell::new(Exchange {
                query: Vec::new(),
                reply: Vec::new(),
                results: Vec::new(),
            });
            let port = unsafe {
                swr_v1_clients_dns_server((&raw const exchange).cast_mut().cast(), collect)
            };
            (exchange.into_inner().results.concat(), port)
        };
        assert_eq!(unsafe { swr_v1_clients_load_resolv_conf(path.as_ptr()) }, 1);
        assert_eq!(server(), ("10.0.0.53".to_string(), 53));
        for (text, expected) in [
            ("127.0.0.1:5353", ("127.0.0.1", 5353)),
            ("127.0.0.1:808088", ("127.0.0.1", 53)),
            ("127.0.0.1:0", ("127.0.0.1", 53)),
            ("127.0.0.1: 99", ("127.0.0.1", 99)),
            ("127.0.0.1:12ab", ("127.0.0.1", 12)),
            ("127.0.0.1:4294967349", ("127.0.0.1", 53)),
            ("::1", ("", 53)),
            ("10.0.0.1\0:99", ("10.0.0.1", 53)),
        ] {
            unsafe { swr_v1_clients_set_dns_server(text.as_ptr(), text.len()) };
            assert_eq!(server(), (expected.0.to_string(), expected.1), "{text}");
        }
        assert_eq!(
            unsafe { swr_v1_clients_load_resolv_conf(c"/nonexistent/resolv.conf".as_ptr()) },
            -1
        );
        assert_eq!(
            server(),
            ("10.0.0.1".to_string(), 53),
            "a failed read keeps the server"
        );

        let hosts = dir.join("hosts");
        std::fs::write(&hosts, "127.0.0.1 localhost\n10.1.1.1 app.local # app\n#10.9.9.9 off.local\n10.2.2.2 last.local").unwrap();
        let path = hosts.to_str().unwrap();
        unsafe { swr_v1_clients_set_hosts_path(path.as_ptr(), path.len()) };
        let spaces: Vec<u8> = (0..=255u8)
            .map(|byte| u8::from(byte.is_ascii_whitespace() || byte == 0x0b))
            .collect();
        let lookup = |name: &str| {
            let exchange = RefCell::new(Exchange {
                query: Vec::new(),
                reply: Vec::new(),
                results: Vec::new(),
            });
            let context = (&raw const exchange).cast_mut().cast();
            let found = unsafe {
                swr_v1_clients_hosts_lookup(
                    name.as_ptr(),
                    name.len(),
                    spaces.as_ptr(),
                    context,
                    collect,
                )
            };
            (found, exchange.into_inner().results)
        };
        assert_eq!(lookup("app.local"), (true, vec!["10.1.1.1".to_string()]));
        assert_eq!(lookup("last.local"), (true, vec!["10.2.2.2".to_string()]));
        assert!(!lookup("off.local").0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
