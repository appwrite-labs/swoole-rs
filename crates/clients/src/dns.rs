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
    ResolvLine, dns_endpoint, domain_decode, domain_encode, hosts_match, resolv_line, valid_port,
};
use std::ffi::{CStr, c_char, c_int};
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

/// The socket side of one DNS query.
pub trait DnsExchange {
    /// Sends the query to the DNS server; false on failure.
    fn send(&mut self, data: &[u8]) -> bool;
    /// Receives into `buffer`: the length, 0/-1 on failure, `-ECANCELED` if the
    /// coroutine was cancelled.
    fn recv(&mut self, buffer: &mut [u8]) -> isize;
    /// Collects one address of the answer in text form.
    fn push(&mut self, address: &[u8]);
}

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
pub fn set_dns_server(server: &[u8]) {
    let (host, digits) = dns_endpoint(server);
    let mut port = DNS_PORT;
    if let Some(digits) = digits {
        let parsed = swoole_rs_corebase::atoi(digits);
        if valid_port(parsed, true) {
            port = parsed;
        }
    }
    let mut config = config();
    config.host = host.to_vec();
    config.port = port;
}

/// `swoole_load_resolv_conf`'s read: the first `nameserver` of the file at `path`
/// becomes the DNS server. 1 found, 0 none, -1 the file did not open (warned with
/// open's errno).
#[must_use]
pub fn load_resolv_conf(path: &CStr) -> c_int {
    match resolv_conf(path) {
        Err(_) => {
            let path = path.to_string_lossy();
            log::sys_warning(
                c"swoole_load_resolv_conf",
                format_args!("fopen({path}) failed"),
            );
            -1
        }
        Ok(None) => 0,
        Ok(Some(server)) => {
            set_dns_server(&server);
            1
        }
    }
}

/// `swoole_set_hosts_path(path)`.
pub fn set_hosts_path(path: &[u8]) {
    config().hosts = path.to_vec();
}

/// Queries `domain` for `family`'s address records with `request_id` over
/// `exchange`, pushing each address of the answer; 0, [`INVALID_DOMAIN`], or the
/// code native sets as the last error (also when the answer has no address).
/// `signed_char` is native `char`'s signedness.
pub fn query(
    domain: &CStr,
    family: c_int,
    request_id: u16,
    exchange: &mut impl DnsExchange,
    signed_char: bool,
) -> c_int {
    let mut packet = vec![0u8; PACKET];
    let [id_high, id_low] = request_id.to_be_bytes();
    // ID, RD, one question.
    put(&mut packet, 0, &[id_high, id_low, 1, 0, 0, 1]);

    let name = domain.to_bytes();
    let capacity = PACKET - HEADER;
    if name.len() > capacity - 3 {
        return INVALID_DOMAIN;
    }
    let question = packet.get_mut(HEADER..).unwrap_or_default();
    if domain_encode(name, question) < 0 {
        return INVALID_DOMAIN;
    }
    let mut steps = HEADER + c_len(question) + 1;
    let [type_high, type_low] = if family == libc::AF_INET6 { AAAA } else { A }.to_be_bytes();
    // QTYPE, QCLASS IN.
    put(&mut packet, steps, &[type_high, type_low, 0, 1]);
    steps += 4;
    let query_initialized = PACKET.min(steps.max(HEADER + name.len() + 3));

    if !exchange.send(packet.get(..steps).unwrap_or_default()) {
        return RESOLVE_FAILED;
    }
    let received = exchange.recv(packet.get_mut(..PACKET - 1).unwrap_or_default());
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

    domain_decode(
        packet.get_mut(HEADER..initialized).unwrap_or_default(),
        signed_char,
    );
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
        let text = swr_v1_clients_ip_text(version, data);
        let length = usize::try_from(text.length).unwrap_or(0);
        exchange.push(text.bytes.get(..length).unwrap_or_default());
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

impl<S: DnsSocket> DnsExchange for Exchange<S> {
    fn send(&mut self, data: &[u8]) -> bool {
        let host = CStr::from_bytes_until_nul(&self.host).unwrap_or_default();
        self.socket.sendto(host, self.port, data)
    }

    fn recv(&mut self, buffer: &mut [u8]) -> isize {
        self.socket.recv(buffer)
    }

    fn push(&mut self, address: &[u8]) {
        self.found.push(address.to_vec());
    }
}

/// The DNS server's host (empty until set or read from resolv.conf), NUL-terminated,
/// and port.
#[must_use]
pub fn server() -> (Vec<u8>, c_int) {
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
        if load_resolv_conf(RESOLV_CONF) <= 0 {
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
    let error = query(
        domain,
        family,
        REQUEST_ID.fetch_add(1, Ordering::Relaxed),
        &mut exchange,
        c_char::MIN != 0,
    );
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

/// The first `nameserver` of the file at `path`, read in `fgets` lines of up to 99
/// bytes: the server, if one (at most 15 bytes, nonempty) is named.
fn resolv_conf(path: &CStr) -> std::io::Result<Option<Vec<u8>>> {
    let path = <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(path.to_bytes());
    let mut file = std::fs::File::open(path)?;
    let mut contents = Vec::new();
    // A failed read (a directory, say) ends `fgets` like the end of the file.
    if std::io::Read::read_to_end(&mut file, &mut contents).is_err() {
        contents.clear();
    }
    let mut rest = contents.as_slice();
    while !rest.is_empty() {
        let end = rest
            .iter()
            .take(99)
            .position(|&byte| byte == b'\n')
            .map_or(rest.len().min(99), |newline| newline + 1);
        let (text, next) = rest.split_at(end);
        rest = next;
        let text = crate::c_str(text);
        return Ok(match resolv_line(text) {
            ResolvLine::Other => continue,
            ResolvLine::Nameserver(token) if !token.is_empty() => Some(token.to_vec()),
            ResolvLine::Nameserver(_) | ResolvLine::Invalid => None,
        });
    }
    Ok(None)
}

/// `get_ip_by_hosts`: the address `domain` has in the hosts file, as text, if
/// any. Lines split as `std::getline` does, cut at the first `#`; `spaces` is
/// the locale's space table.
#[must_use]
pub fn hosts_lookup(domain: &[u8], spaces: &[u8; 256]) -> Option<Vec<u8>> {
    let mut path = config().hosts.clone();
    if path.is_empty() {
        path = HOSTS.to_vec();
    }
    // `std::ifstream` opens the path up to its first NUL.
    path.truncate(c_len(&path));
    let Ok(contents) =
        std::fs::read(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&path))
    else {
        return None;
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
        if let Some(address) = hosts_match(&line, domain, spaces) {
            return Some(address.to_vec());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Exchange {
        query: Vec<u8>,
        reply: Vec<u8>,
        results: Vec<String>,
        cancelled: bool,
    }

    impl DnsExchange for Exchange {
        fn send(&mut self, data: &[u8]) -> bool {
            self.query = data.to_vec();
            true
        }
        fn recv(&mut self, out: &mut [u8]) -> isize {
            if self.cancelled {
                return -isize::try_from(libc::ECANCELED).unwrap();
            }
            let id = [self.query[0], self.query[1]];
            self.reply[..2].copy_from_slice(&id);
            let n = self.reply.len().min(out.len());
            out[..n].copy_from_slice(&self.reply[..n]);
            isize::try_from(n).unwrap()
        }
        fn push(&mut self, text: &[u8]) {
            self.results.push(String::from_utf8(text.to_vec()).unwrap());
        }
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

    fn ask(family: c_int, reply: Vec<u8>, cancelled: bool) -> (c_int, Exchange) {
        let mut exchange = Exchange {
            query: Vec::new(),
            reply,
            results: Vec::new(),
            cancelled,
        };
        let error = query(c"example.test", family, 7, &mut exchange, true);
        (error, exchange)
    }

    #[test]
    fn asks_once_and_reads_addresses() {
        let cname: &[u8] = b"\x03www\xc0\x0c";
        let (error, exchange) = ask(
            libc::AF_INET,
            reply(&[(5, cname), (A, &[192, 0, 2, 1]), (A, &[192, 0, 2, 2])]),
            false,
        );
        assert_eq!(error, 0);
        assert_eq!(exchange.results, ["192.0.2.1", "192.0.2.2"]);
        assert_eq!(&exchange.query[..12], &[0, 7, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            &exchange.query[12..],
            b"\x07example\x04test\x00\x00\x01\x00\x01"
        );

        let six = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let (error, exchange) = ask(libc::AF_INET6, reply(&[(AAAA, &six)]), false);
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
            ask(libc::AF_INET, reply(&[(5, cname)]), false).0,
            RESOLVE_FAILED
        );
        assert_eq!(ask(libc::AF_INET, reply(&[]), true).0, CANCELED);
        let mut truncated = reply(&[(A, &[192, 0, 2, 1])]);
        truncated[7] = 200;
        assert_eq!(
            ask(libc::AF_INET, truncated, false).1.results,
            ["192.0.2.1"]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no gethostbyname2_r")]
    fn resolves_through_libc() {
        let addr = swoole_rs_corebase::gethostbyname(libc::AF_INET, c"localhost");
        assert_eq!(
            addr.unwrap()[..4]
                .iter()
                .map(|&b| b.to_ne_bytes()[0])
                .collect::<Vec<_>>(),
            [127, 0, 0, 1]
        );
        assert!(swoole_rs_corebase::gethostbyname(libc::AF_UNIX, c"localhost").is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strtol")]
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
            let (mut host, port) = server();
            assert_eq!(host.pop(), Some(0));
            (String::from_utf8(host).unwrap(), port)
        };
        assert_eq!(load_resolv_conf(&path), 1);
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
            set_dns_server(text.as_bytes());
            assert_eq!(server(), (expected.0.to_string(), expected.1), "{text}");
        }
        assert_eq!(load_resolv_conf(c"/nonexistent/resolv.conf"), -1);
        assert_eq!(
            server(),
            ("10.0.0.1".to_string(), 53),
            "a failed read keeps the server"
        );

        let hosts = dir.join("hosts");
        std::fs::write(&hosts, "127.0.0.1 localhost\n10.1.1.1 app.local # app\n#10.9.9.9 off.local\n10.2.2.2 last.local").unwrap();
        let path = hosts.to_str().unwrap();
        set_hosts_path(path.as_bytes());
        let spaces: [u8; 256] = core::array::from_fn(|byte| {
            let byte = u8::try_from(byte).unwrap();
            u8::from(byte.is_ascii_whitespace() || byte == 0x0b)
        });
        let lookup = |name: &str| hosts_lookup(name.as_bytes(), &spaces);
        assert_eq!(lookup("app.local"), Some(b"10.1.1.1".to_vec()));
        assert_eq!(lookup("last.local"), Some(b"10.2.2.2".to_vec()));
        assert_eq!(lookup("off.local"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
