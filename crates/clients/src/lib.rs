//! `network::Client`, DNS resolution, address policy and the persistent pool.
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

pub mod address;
#[cfg(feature = "cares")]
mod cares;
pub mod client;
pub mod dns;
pub mod http_format;
pub mod ip;
pub mod pool;
pub mod resolver;
pub mod ws;

use std::ffi::c_int;
use std::ops::Range;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::net_address::{
    TCP, TCP6, UDP, UDP6, UNIX_DGRAM, UNIX_STREAM, c_str, format_into,
};

/// `dns_lookup_impl_with_cares(domain, family, timeout)` when built with c-ares
/// (`--enable-cares`), else `None`.
#[must_use]
pub fn cares_lookup(domain: &std::ffi::CStr, family: c_int, timeout: f64) -> Option<Vec<Vec<u8>>> {
    #[cfg(feature = "cares")]
    return Some(cares::lookup(domain, family, timeout));
    #[cfg(not(feature = "cares"))]
    {
        let _ = (domain, family, timeout);
        None
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UrlPlan {
    /// 0 invalid, 1 IPv4 or hostname, 2 IPv6, 3 Unix.
    kind: u32,
    datagram: bool,
    host_offset: usize,
    host_length: usize,
    port_offset: usize,
}

fn url_plan(url: &[u8]) -> UrlPlan {
    let (rest, offset, datagram, unix) = if let Some(rest) = url.strip_prefix(b"unix://") {
        (rest, 7, false, true)
    } else if let Some(rest) = url.strip_prefix(b"udg://") {
        (rest, 6, true, true)
    } else if let Some(rest) = url.strip_prefix(b"tcp://") {
        (rest, 6, false, false)
    } else if let Some(rest) = url.strip_prefix(b"udp://") {
        (rest, 6, true, false)
    } else {
        return UrlPlan::default();
    };
    let mut plan = UrlPlan {
        datagram,
        host_offset: offset,
        ..UrlPlan::default()
    };
    if unix {
        // Pinned regex is /[^?#]+: a solitary slash does not match.
        if rest.len() >= 2
            && rest.starts_with(b"/")
            && !rest.iter().any(|b| matches!(b, b'?' | b'#'))
        {
            plan.kind = 3;
            plan.host_length = rest.len();
        }
        return plan;
    }
    if rest.first() == Some(&b'[') {
        let Some(end) = rest.iter().position(|&b| b == b']') else {
            return UrlPlan::default();
        };
        if end <= 1 || rest.get(end + 1) != Some(&b':') || end + 2 == rest.len() {
            return UrlPlan::default();
        }
        plan.kind = 2;
        plan.host_offset += 1;
        plan.host_length = end - 1;
        plan.port_offset = offset + end + 2;
    } else {
        let Some(end) = rest.iter().position(|&b| b == b':') else {
            return UrlPlan::default();
        };
        if end == 0 || rest.iter().take(end).any(|&b| b == b'[') || end + 1 == rest.len() {
            return UrlPlan::default();
        }
        plan.kind = 1;
        plan.host_length = end;
        plan.port_offset = offset + end + 1;
    }
    plan
}

/// `Address::assign(url)` returns this for a URL outside its grammar (native logs it)
/// and the next for a port past `int` (`std::stoi` throws `std::out_of_range`).
pub const URL_INVALID: c_int = -1;
pub const URL_PORT_RANGE: c_int = -2;

/// The socket type, host span and port `Address::assign(url)` assigns for a
/// `unix://`, `udg://`, `tcp://` or `udp://` URL.
///
/// # Errors
/// `URL_INVALID` outside the grammar, `URL_PORT_RANGE` for a port past `int`.
pub fn url(url: &[u8]) -> Result<(c_int, Range<usize>, c_int), c_int> {
    let plan = url_plan(url);
    let host = plan.host_offset..plan.host_offset + plan.host_length;
    match (plan.kind, plan.datagram) {
        (0, _) => Err(URL_INVALID),
        (3, datagram) => Ok((if datagram { UNIX_DGRAM } else { UNIX_STREAM }, host, 0)),
        (family, datagram) => {
            let digits = url.get(plan.port_offset..).unwrap_or_default();
            if !digits.iter().all(u8::is_ascii_digit) {
                return Err(URL_INVALID);
            }
            let port = digits.iter().try_fold(0_i32, |n, &digit| {
                n.checked_mul(10)?.checked_add(i32::from(digit - b'0'))
            });
            let kind = match (family, datagram) {
                (2, true) => UDP6,
                (2, false) => TCP6,
                (_, true) => UDP,
                (_, false) => TCP,
            };
            Ok((kind, host, port.ok_or(URL_PORT_RANGE)?))
        }
    }
}

/// [`url`] for native: `outcome` 0 with the rest set, or a `URL_*` outcome.
#[repr(C)]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct UrlAssign {
    pub outcome: c_int,
    pub kind: c_int,
    pub host_offset: usize,
    pub host_length: usize,
    pub port: c_int,
}

/// # Safety
/// `data` denotes `length` readable bytes, or is null for 0. No pointer is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_url(data: *const u8, length: usize) -> UrlAssign {
    if data.is_null() || length == 0 {
        return UrlAssign {
            outcome: URL_INVALID,
            ..UrlAssign::default()
        };
    }
    // SAFETY: the caller lends `length` readable bytes.
    match url(unsafe { std::slice::from_raw_parts(data, length) }) {
        Ok((kind, host, port)) => UrlAssign {
            outcome: 0,
            kind,
            host_offset: host.start,
            host_length: host.len(),
            port,
        },
        Err(outcome) => UrlAssign {
            outcome,
            ..UrlAssign::default()
        },
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_port(port: i32, for_connect: u8) -> u8 {
    u8::from((0..=65535).contains(&port) && (port != 0 || for_connect == 0))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostMatch {
    pub found: u32,
    pub offset: usize,
    pub length: usize,
}

/// The address field of a hosts-file `line` naming `search`, its fields split by
/// the 256-entry `spaces` table.
pub(crate) fn hosts_match(line: &[u8], search: &[u8], spaces: &[u8]) -> HostMatch {
    if matches!(line.first(), None | Some(0 | b'\r' | b'\n')) {
        return HostMatch::default();
    }
    let mut tokens = line
        .split(|b| spaces.get(usize::from(*b)).is_some_and(|&space| space != 0))
        .filter(|token| !token.is_empty());
    let Some(address) = tokens.next() else {
        return HostMatch::default();
    };
    if tokens.any(|token| token == search) {
        HostMatch {
            found: 1,
            offset: address.as_ptr() as usize - line.as_ptr() as usize,
            length: address.len(),
        }
    } else {
        HostMatch::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CookiePlan {
    pub key_length: usize,
    pub value_offset: usize,
    pub value_length: usize,
}

/// `php_swoole_http_parse_set_cookies`' split of a `Set-Cookie` value: the key's
/// length and the value's offset and length.
#[must_use]
pub fn cookie(bytes: &[u8]) -> CookiePlan {
    let length = bytes.len();
    if length == 0 {
        return CookiePlan::default();
    }
    let equals = bytes.iter().position(|&b| b == b'=');
    let value_offset = equals.map_or(0, |offset| offset + 1);
    CookiePlan {
        key_length: equals.unwrap_or(0),
        value_offset,
        value_length: bytes
            .iter()
            .skip(value_offset)
            .position(|&b| b == b';')
            .unwrap_or(length - value_offset),
    }
}

/// `domain_encode`: `input` as DNS labels into `output`; -1 when `output` cannot
/// hold `input.len() + 3` bytes.
pub fn domain_encode(input: &[u8], output: &mut [u8]) -> i32 {
    let Some(output) = input
        .len()
        .checked_add(3)
        .and_then(|required| output.get_mut(..required))
    else {
        return -1;
    };
    // `[len]label[len]label` with each dot turned into the next label's length
    // byte, then two NULs.
    let mut start = 0;
    for label in input.split(|&byte| byte == b'.') {
        // Upstream's (char) length byte: a label over 255 bytes wraps, as in C.
        let length_byte = label.len().c_cast::<u8>();
        let slot = output.get_mut(start..=start + label.len());
        let Some((head, body)) = slot.and_then(<[u8]>::split_first_mut) else {
            return -1;
        };
        *head = length_byte;
        body.copy_from_slice(label);
        start += label.len() + 1;
    }
    output.get_mut(start..).unwrap_or_default().fill(0);
    0
}

/// `domain_decode` over `bytes`, whose plain `char` is signed when `signed_char`;
/// false where a read or write would leave `bytes`.
pub fn domain_decode(bytes: &mut [u8], signed_char: bool) -> bool {
    let capacity = bytes.len();
    if capacity == 0 {
        return false;
    }
    let mut index = 0;
    loop {
        let Some(end) = bytes.iter().position(|&byte| byte == 0) else {
            return false;
        };
        if index >= end {
            break;
        }
        let Some(&byte) = bytes.get(index) else {
            return false;
        };
        let length = if signed_char {
            i32::from(byte.cast_signed()).cast_unsigned()
        } else {
            u32::from(byte)
        };
        for _ in 0..length {
            let Some(next) = index.checked_add(1).filter(|&next| next < capacity) else {
                return false;
            };
            bytes.copy_within(next..=next, index);
            index = next;
        }
        let Some(slot) = bytes.get_mut(index) else {
            return false;
        };
        *slot = b'.';
        index += 1;
    }
    let Some(last) = index.checked_sub(1).and_then(|last| bytes.get_mut(last)) else {
        return false;
    };
    *last = 0;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_unix_grammar_and_binary_host() {
        assert_eq!(url_plan(b"unix:///").kind, 0);
        assert_eq!(url_plan(b"unix:////").kind, 3);
        assert_eq!(url_plan(b"unix:///a?b").kind, 0);
        let p = url_plan(b"tcp://a\0b:12");
        assert_eq!(p.kind, 1);
        assert_eq!(p.host_length, 3);
        assert_eq!(&b"tcp://a\0b:12"[p.port_offset..], b"12");
    }
    #[test]
    fn brackets_and_empty_fields() {
        assert_eq!(url_plan(b"tcp://[::1]:80").kind, 2);
        for url in [
            b"tcp://[]:80".as_slice(),
            b"tcp://a:",
            b"tcp://:80",
            b"tcp://[a:80",
        ] {
            assert_eq!(url_plan(url).kind, 0);
        }
    }
    #[test]
    fn url_outcomes() {
        let url = |url: &[u8]| unsafe { swr_v1_clients_url(url.as_ptr(), url.len()) };
        let plan = url(b"tcp://127.0.0.1:080");
        assert_eq!(
            (
                plan.outcome,
                plan.kind,
                plan.host_offset,
                plan.host_length,
                plan.port
            ),
            (0, TCP, 6, 9, 80)
        );
        assert_eq!(url(b"tcp://1.2.3.4:8x").outcome, URL_INVALID);
        assert_eq!(url(b"tcp://1.2.3.4:99999999999").outcome, URL_PORT_RANGE);
        assert_eq!(url(b"tcp://1.2.3.4:2147483647").port, i32::MAX);
        assert_eq!(url(b"udp://[::1]:53").kind, UDP6);
        assert_eq!(url(b"udg:///tmp/a.sock").kind, UNIX_DGRAM);
        assert_eq!(url(b"http://a:1").outcome, URL_INVALID);
    }
    #[test]
    fn port() {
        assert_eq!(swr_v1_clients_port(0, 0), 1);
        assert_eq!(swr_v1_clients_port(0, 1), 0);
        assert_eq!(swr_v1_clients_port(65536, 0), 0);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolvLine {
    /// 0: no prefix; 1: defined native token pair; 2: outside the compared domain.
    pub kind: u32,
    pub offset: usize,
    pub length: usize,
}

pub(crate) fn resolv_line(line: &[u8]) -> ResolvLine {
    let line = c_str(line);
    if !line.starts_with(b"nameserver") {
        return ResolvLine::default();
    }
    let invalid = ResolvLine {
        kind: 2,
        ..ResolvLine::default()
    };
    // Native strtok first uses only SPACE, then only LF. Extra spaces/tabs and
    // CR in the second token are significant; this is not a new config grammar.
    let Some(separator) = line.iter().position(|&byte| byte == b' ') else {
        return invalid;
    };
    if separator > 15 {
        return invalid;
    }
    let offset = separator + 1;
    let rest = line.get(offset..).unwrap_or_default();
    let Some(start) = rest.iter().position(|&byte| byte != b'\n') else {
        return invalid;
    };
    let rest = rest.get(start..).unwrap_or_default();
    let length = rest
        .iter()
        .position(|&byte| byte == b'\n')
        .unwrap_or(rest.len());
    if length > 15 {
        return invalid;
    }
    ResolvLine {
        kind: 1,
        offset: offset + start,
        length,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DnsEndpoint {
    pub has_port: u32,
    pub host_length: usize,
    pub port_offset: usize,
}

/// The host and port spans of a `host[:port]` DNS server, up to its first NUL.
pub(crate) fn dns_endpoint(bytes: &[u8]) -> DnsEndpoint {
    let bytes = c_str(bytes);
    match bytes.iter().position(|&byte| byte == b':') {
        Some(colon) => DnsEndpoint {
            has_port: 1,
            host_length: colon,
            port_offset: colon + 1,
        },
        None => DnsEndpoint {
            host_length: bytes.len(),
            ..DnsEndpoint::default()
        },
    }
}
