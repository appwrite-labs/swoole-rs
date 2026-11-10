//! Numeric address grammar over borrowed initialized bytes. Native sockaddr
//! layouts, publication and resolver/errno effects stay outside Rust.
use std::net::{Ipv4Addr, Ipv6Addr};
use swoole_rs_corebase::net_address::{ipv4, ipv6};
#[repr(C)]
pub struct ClientsIp {
    pub bytes: [u8; 16],
    pub length: u32,
}
/// # Safety
/// Input denotes `length` initialized readable bytes, or null for length0. Native
/// `std::string` callers pass their full bytes; parsing stops at the first NUL to
/// preserve their original `c_str/inet_pton` boundary. No pointer is retained.
#[must_use]
pub unsafe fn swr_v1_clients_ip(
    version: u8,
    data: *const u8,
    length: usize,
    allow_zeroes: u8,
) -> ClientsIp {
    let mut result = ClientsIp {
        bytes: [0; 16],
        length: 0,
    };
    if data.is_null() || length > isize::MAX as usize {
        return result;
    }
    // SAFETY: `data` is non-null and `length <= isize::MAX` (checked above); the caller lends `length` initialized bytes for this call.
    let input = unsafe { std::slice::from_raw_parts(data, length) };
    let Ok(text) = std::str::from_utf8(crate::c_str(input)) else {
        return result;
    };
    match version {
        4 => {
            if let Ok(ip) = ipv4(text.as_bytes(), allow_zeroes != 0).ok_or(()) {
                result.bytes[..4].copy_from_slice(&ip.octets());
                result.length = 4;
            }
        }
        6 => {
            if let Ok(ip) = ipv6(text, allow_zeroes != 0).ok_or(()) {
                result.bytes = ip.octets();
                result.length = 16;
            }
        }
        _ => {}
    }
    result
}
/// # Safety
/// Address denotes exactly4 (version4) or16 (version6) initialized bytes. No
/// pointer is retained. Original IPv4 policy recognizes only127.0.0.1, not /8.
#[must_use]
pub unsafe fn swr_v1_clients_loopback(version: u8, address: *const u8) -> u8 {
    let length = match version {
        4 => 4,
        6 => 16,
        _ => return 0,
    };
    if address.is_null() {
        return 0;
    }
    // SAFETY: non-null (checked above); the caller lends the 4- or 16-byte address for `version`.
    let bytes = unsafe { std::slice::from_raw_parts(address, length) };
    u8::from(if version == 4 {
        bytes == [127, 0, 0, 1]
    } else {
        bytes == Ipv6Addr::LOCALHOST.octets()
    })
}

#[repr(C)]
pub struct ClientsIpText {
    pub bytes: [u8; 40],
    pub length: u32,
}
/// # Safety
/// Input denotes4 (version4) or16 (version6) initialized address bytes. No pointer
/// is retained. DNS IPv6 output deliberately keeps all eight four-digit groups.
#[must_use]
pub unsafe fn swr_v1_clients_ip_text(version: u8, address: *const u8) -> ClientsIpText {
    let mut output = ClientsIpText {
        bytes: [0; 40],
        length: 0,
    };
    if address.is_null() {
        return output;
    }
    let mut text = [0u8; 40];
    let written = match version {
        4 => {
            // SAFETY: non-null (checked above); the caller lends the 4-byte address.
            let v4 = unsafe { address.cast::<[u8; 4]>().read_unaligned() };
            crate::format_into(&mut text, format_args!("{}", Ipv4Addr::from(v4)))
        }
        6 => {
            // SAFETY: non-null (checked above); the caller lends the 16-byte address.
            let v6 = unsafe { address.cast::<[u8; 16]>().read_unaligned() };
            let [a, b, c, d, e, f, g, h] = Ipv6Addr::from(v6).segments();
            crate::format_into(
                &mut text,
                format_args!("{a:04x}:{b:04x}:{c:04x}:{d:04x}:{e:04x}:{f:04x}:{g:04x}:{h:04x}"),
            )
        }
        _ => None,
    };
    let written = written.unwrap_or_default();
    #[expect(clippy::cast_possible_truncation, reason = "written fits output.bytes")]
    let length = written.len() as u32;
    output.length = length;
    output.bytes = text;
    output
}
