//! Numeric address grammar over borrowed initialized bytes. Native sockaddr
//! layouts, publication and resolver/errno effects stay outside Rust.
use std::net::{Ipv4Addr, Ipv6Addr};
#[repr(C)]
pub struct ClientsIp {
    pub bytes: [u8; 16],
    pub length: u32,
}
fn ipv4(text: &str, allow_zeroes: bool) -> Option<Ipv4Addr> {
    let mut octets = [0u8; 4];
    let mut parts = text.split('.');
    for octet in &mut octets {
        let part = parts.next()?;
        if part.is_empty() || (!allow_zeroes && part.len() > 1 && part.starts_with('0')) {
            return None;
        }
        let mut value = 0u16;
        for byte in part.bytes() {
            if !byte.is_ascii_digit() {
                return None;
            }
            value = value.checked_mul(10)?.checked_add(u16::from(byte - b'0'))?;
            if value > 255 {
                return None;
            }
        }
        *octet = u8::try_from(value).ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(Ipv4Addr::from(octets))
}
fn ipv6(text: &str, allow_zeroes: bool) -> Option<Ipv6Addr> {
    if !text.contains('.') {
        return text.parse().ok();
    }
    let colon = text.rfind(':')?;
    let [a, b, c, d] = ipv4(text.get(colon + 1..)?, allow_zeroes)?.octets();
    // Replace the dotted tail with two hex groups in bounded caller-local
    // storage. This preserves native Apple decimal-leading-zero grammar without
    // introducing an allocation effect into numeric parsing.
    let mut normalized = [0u8; 128];
    let normalized = crate::format_into(
        &mut normalized,
        format_args!(
            "{}{:x}:{:x}",
            text.get(..=colon)?,
            u16::from_be_bytes([a, b]),
            u16::from_be_bytes([c, d])
        ),
    )?;
    std::str::from_utf8(normalized).ok()?.parse().ok()
}
/// # Safety
/// Input denotes `length` initialized readable bytes, or null for length0. Native
/// `std::string` callers pass their full bytes; parsing stops at the first NUL to
/// preserve their original `c_str/inet_pton` boundary. No pointer is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_ip(
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
            if let Ok(ip) = ipv4(text, allow_zeroes != 0).ok_or(()) {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_loopback(version: u8, address: *const u8) -> u8 {
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_ip_text(version: u8, address: *const u8) -> ClientsIpText {
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
