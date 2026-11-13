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

/// What a URL's host names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum UrlHost {
    #[default]
    Invalid,
    /// An IPv4 address or a hostname.
    Named,
    Ipv6,
    Unix,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UrlPlan {
    kind: UrlHost,
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
            plan.kind = UrlHost::Unix;
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
        plan.kind = UrlHost::Ipv6;
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
        plan.kind = UrlHost::Named;
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
        (UrlHost::Invalid, _) => Err(URL_INVALID),
        (UrlHost::Unix, datagram) => Ok((if datagram { UNIX_DGRAM } else { UNIX_STREAM }, host, 0)),
        (family, datagram) => {
            let digits = url.get(plan.port_offset..).unwrap_or_default();
            if !digits.iter().all(u8::is_ascii_digit) {
                return Err(URL_INVALID);
            }
            let port = digits.iter().try_fold(0_i32, |n, &digit| {
                n.checked_mul(10)?.checked_add(i32::from(digit - b'0'))
            });
            let kind = match (family, datagram) {
                (UrlHost::Ipv6, true) => UDP6,
                (UrlHost::Ipv6, false) => TCP6,
                (_, true) => UDP,
                (_, false) => TCP,
            };
            Ok((kind, host, port.ok_or(URL_PORT_RANGE)?))
        }
    }
}

/// Whether `port` is a valid port; 0 is one only when not connecting.
#[must_use]
pub fn valid_port(port: i32, for_connect: bool) -> bool {
    (0..=65535).contains(&port) && (port != 0 || !for_connect)
}

/// The address field of a hosts-file `line` naming `search`, its fields split by
/// the 256-entry `spaces` table.
pub(crate) fn hosts_match<'a>(line: &'a [u8], search: &[u8], spaces: &[u8]) -> Option<&'a [u8]> {
    if matches!(line.first(), None | Some(0 | b'\r' | b'\n')) {
        return None;
    }
    let mut tokens = line
        .split(|b| spaces.get(usize::from(*b)).is_some_and(|&space| space != 0))
        .filter(|token| !token.is_empty());
    let address = tokens.next()?;
    tokens.any(|token| token == search).then_some(address)
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
        assert_eq!(url_plan(b"unix:///").kind, UrlHost::Invalid);
        assert_eq!(url_plan(b"unix:////").kind, UrlHost::Unix);
        assert_eq!(url_plan(b"unix:///a?b").kind, UrlHost::Invalid);
        let p = url_plan(b"tcp://a\0b:12");
        assert_eq!(p.kind, UrlHost::Named);
        assert_eq!(p.host_length, 3);
        assert_eq!(&b"tcp://a\0b:12"[p.port_offset..], b"12");
    }
    #[test]
    fn brackets_and_empty_fields() {
        assert_eq!(url_plan(b"tcp://[::1]:80").kind, UrlHost::Ipv6);
        for url in [
            b"tcp://[]:80".as_slice(),
            b"tcp://a:",
            b"tcp://:80",
            b"tcp://[a:80",
        ] {
            assert_eq!(url_plan(url).kind, UrlHost::Invalid);
        }
    }
    #[test]
    fn url_outcomes() {
        assert_eq!(url(b"tcp://127.0.0.1:080"), Ok((TCP, 6..6 + 9, 80)));
        assert_eq!(url(b"tcp://1.2.3.4:8x"), Err(URL_INVALID));
        assert_eq!(url(b"tcp://1.2.3.4:99999999999"), Err(URL_PORT_RANGE));
        let field = |text: &[u8]| url(text).map(|(kind, _, port)| (kind, port));
        assert_eq!(field(b"tcp://1.2.3.4:2147483647"), Ok((TCP, i32::MAX)));
        assert_eq!(field(b"udp://[::1]:53"), Ok((UDP6, 53)));
        assert_eq!(field(b"udg:///tmp/a.sock"), Ok((UNIX_DGRAM, 0)));
        assert_eq!(url(b"http://a:1"), Err(URL_INVALID));
    }
    #[test]
    fn port() {
        assert!(valid_port(0, false));
        assert!(!valid_port(0, true));
        assert!(!valid_port(65536, false));
    }
}

/// A resolv.conf line, as native strtok splits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolvLine<'a> {
    /// Not a `nameserver` line.
    Other,
    /// A `nameserver` line and its address token.
    Nameserver(&'a [u8]),
    /// A `nameserver` line outside the compared domain.
    Invalid,
}

pub(crate) fn resolv_line(line: &[u8]) -> ResolvLine<'_> {
    let line = c_str(line);
    if !line.starts_with(b"nameserver") {
        return ResolvLine::Other;
    }
    // Native strtok first uses only SPACE, then only LF. Extra spaces/tabs and
    // CR in the second token are significant; this is not a new config grammar.
    let Some(separator) = line.iter().position(|&byte| byte == b' ') else {
        return ResolvLine::Invalid;
    };
    if separator > 15 {
        return ResolvLine::Invalid;
    }
    let rest = line.get(separator + 1..).unwrap_or_default();
    let Some(start) = rest.iter().position(|&byte| byte != b'\n') else {
        return ResolvLine::Invalid;
    };
    let rest = rest.get(start..).unwrap_or_default();
    let token = rest.split(|&byte| byte == b'\n').next().unwrap_or_default();
    if token.len() > 15 {
        return ResolvLine::Invalid;
    }
    ResolvLine::Nameserver(token)
}

/// The host and, after its first colon, the port of a `host[:port]` DNS server,
/// up to its first NUL.
pub(crate) fn dns_endpoint(bytes: &[u8]) -> (&[u8], Option<&[u8]>) {
    let bytes = c_str(bytes);
    match bytes.iter().position(|&byte| byte == b':') {
        Some(colon) => (
            bytes.get(..colon).unwrap_or_default(),
            bytes.get(colon + 1..),
        ),
        None => (bytes, None),
    }
}
