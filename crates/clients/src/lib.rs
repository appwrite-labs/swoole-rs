//! `network::Client`, DNS resolution, address policy and the persistent pool.
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
pub mod ws;

use address::{TCP, TCP6, UDP, UDP6, UNIX_DGRAM, UNIX_STREAM};
use std::ffi::c_int;
use std::ops::Range;

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
        if rest.len() >= 2 && rest[0] == b'/' && !rest.iter().any(|b| matches!(b, b'?' | b'#')) {
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
        if end == 0 || rest[..end].contains(&b'[') || end + 1 == rest.len() {
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
            let digits = &url[plan.port_offset..];
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostMatch {
    pub found: u32,
    pub offset: usize,
    pub length: usize,
}

fn hosts_match(line: &[u8], search: &[u8], spaces: &[u8]) -> HostMatch {
    if line.is_empty() || matches!(line[0], 0 | b'\r' | b'\n') {
        return HostMatch::default();
    }
    let mut tokens = line
        .split(|b| spaces[usize::from(*b)] != 0)
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

/// # Safety
/// Nonempty line/search pointers denote initialized spans no larger than `isize::MAX`;
/// spaces denotes256 initialized classification bytes. None is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_hosts_match(
    line: *const u8,
    line_length: usize,
    search: *const u8,
    search_length: usize,
    spaces: *const u8,
) -> HostMatch {
    if line_length == 0
        || line.is_null()
        || spaces.is_null()
        || line_length > isize::MAX as usize
        || search_length > isize::MAX as usize
        || (search_length != 0 && search.is_null())
    {
        return HostMatch::default();
    }
    // SAFETY: `line` is non-null and `line_length <= isize::MAX` (checked above); the caller lends `line_length` initialized bytes for this call.
    let line = unsafe { std::slice::from_raw_parts(line, line_length) };
    let search = if search_length == 0 {
        &[]
    } else {
        // SAFETY: `search` is non-null and `search_length <= isize::MAX` (checked above); the caller lends `search_length` initialized bytes for this call.
        unsafe { std::slice::from_raw_parts(search, search_length) }
    };
    // SAFETY: non-null (checked above); the caller lends its 256-entry class table.
    let spaces = unsafe { std::slice::from_raw_parts(spaces, 256) };
    hosts_match(line, search, spaces)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CookiePlan {
    pub key_length: usize,
    pub value_offset: usize,
    pub value_length: usize,
}

/// # Safety
/// data denotes length initialized bytes, or is null for length0; no pointer retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_cookie(data: *const u8, length: usize) -> CookiePlan {
    if length == 0 || data.is_null() || length > isize::MAX as usize {
        return CookiePlan::default();
    }
    // SAFETY: `data` is non-null and `length <= isize::MAX` (checked above); the caller lends `length` initialized bytes for this call.
    let bytes = unsafe { std::slice::from_raw_parts(data, length) };
    let equals = bytes.iter().position(|&b| b == b'=');
    let value_offset = equals.map_or(0, |offset| offset + 1);
    CookiePlan {
        key_length: equals.unwrap_or(0),
        value_offset,
        value_length: bytes[value_offset..]
            .iter()
            .position(|&b| b == b';')
            .unwrap_or(length - value_offset),
    }
}

/// # Safety
/// source denotes length initialized bytes; output denotes capacity writable bytes.
/// The native caller guarantees the two spans do not overlap. No pointer retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_domain_encode(
    source: *const u8,
    length: usize,
    output: *mut u8,
    capacity: usize,
) -> i32 {
    let Some(required) = length.checked_add(3) else {
        return -1;
    };
    if required > capacity
        || capacity > isize::MAX as usize
        || output.is_null()
        || (length != 0 && source.is_null())
    {
        return -1;
    }
    let input = if length == 0 {
        &[]
    } else {
        // SAFETY: `source` is non-null and `length <= isize::MAX` (checked above); the caller lends `length` initialized bytes for this call.
        unsafe { std::slice::from_raw_parts(source, length) }
    };
    // SAFETY: `output` is non-null and `required <= isize::MAX` (checked above); the caller lends `required` writable bytes for this call.
    let output = unsafe { std::slice::from_raw_parts_mut(output, required) };
    output[1..=length].copy_from_slice(input);
    output[length + 1] = b'.';
    output[length + 2] = 0;
    let mut position = 0;
    for (index, byte) in input
        .iter()
        .copied()
        .chain(std::iter::once(b'.'))
        .enumerate()
    {
        if byte == b'.' {
            let label_length = index - position;
            // Upstream's (char) length byte: a label over 255 bytes wraps, as in C.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "upstream's (char) label length"
            )]
            let length_byte = label_length as u8;
            output[position] = length_byte;
            position += label_length + 1;
        }
    }
    output[position] = 0;
    0
}

/// # Safety
/// data denotes capacity initialized writable bytes. `signed_char` records native
/// plain-char promotion. No pointer retained; every read/write stays in this span.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_domain_decode(
    data: *mut u8,
    capacity: usize,
    signed_char: u8,
) -> u8 {
    if data.is_null() || capacity == 0 || capacity > isize::MAX as usize {
        return 0;
    }
    // SAFETY: `data` is non-null and `capacity <= isize::MAX` (checked above); the caller lends `capacity` writable bytes for this call.
    let bytes = unsafe { std::slice::from_raw_parts_mut(data, capacity) };
    let mut index = 0;
    loop {
        let Some(end) = bytes.iter().position(|&byte| byte == 0) else {
            return 0;
        };
        if index >= end {
            break;
        }
        let length = if signed_char != 0 {
            i32::from(bytes[index].cast_signed()).cast_unsigned()
        } else {
            u32::from(bytes[index])
        };
        for _ in 0..length {
            let Some(next) = index.checked_add(1).filter(|&next| next < capacity) else {
                return 0;
            };
            bytes[index] = bytes[next];
            index = next;
        }
        if index >= capacity {
            return 0;
        }
        bytes[index] = b'.';
        index += 1;
    }
    if index == 0 {
        return 0;
    }
    bytes[index - 1] = 0;
    1
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolvLine {
    /// 0: no prefix; 1: defined native token pair; 2: outside the compared domain.
    pub kind: u32,
    pub offset: usize,
    pub length: usize,
}

fn resolv_line(line: &[u8]) -> ResolvLine {
    let line = &line[..line
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(line.len())];
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
    let rest = &line[offset..];
    let start = rest
        .iter()
        .position(|&byte| byte != b'\n')
        .unwrap_or(rest.len());
    if start == rest.len() {
        return invalid;
    }
    let rest = &rest[start..];
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

/// # Safety
/// Input denotes initialized line bytes. No pointer is retained. Kind2 records
/// inputs where the original null strtok/copy overflow has no defined parity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_resolv_line(data: *const u8, length: usize) -> ResolvLine {
    if data.is_null() || length > isize::MAX as usize {
        return ResolvLine::default();
    }
    // SAFETY: `data` is non-null and `length <= isize::MAX` (checked above); the caller lends `length` initialized bytes for this call.
    resolv_line(unsafe { std::slice::from_raw_parts(data, length) })
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DnsEndpoint {
    pub has_port: u32,
    pub host_length: usize,
    pub port_offset: usize,
}

/// # Safety
/// Input is an initialized borrowed string span. Native strtol owns locale,
/// errno and long-to-int narrowing; only C-string component spans enter Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_dns_endpoint(
    data: *const u8,
    length: usize,
) -> DnsEndpoint {
    if data.is_null() || length > isize::MAX as usize {
        return DnsEndpoint::default();
    }
    // SAFETY: `data` is non-null and `length <= isize::MAX` (checked above); the caller lends `length` initialized bytes for this call.
    let bytes = unsafe { std::slice::from_raw_parts(data, length) };
    let bytes = &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())];
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
