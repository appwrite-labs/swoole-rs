//! Numeric address grammar over borrowed initialized bytes. Native sockaddr
//! layouts, publication and resolver/errno effects stay outside Rust.
use std::net::{Ipv4Addr, Ipv6Addr};
use swoole_rs_corebase::net_address::{ipv4, ipv6};
#[repr(C)]
pub struct ClientsIp {
    pub bytes: [u8; 16],
    pub length: u32,
}
/// Native `std::string` callers pass their full bytes; parsing stops at the first
/// NUL to preserve their original `c_str/inet_pton` boundary.
#[must_use]
pub fn swr_v1_clients_ip(version: u8, input: &[u8], allow_zeroes: u8) -> ClientsIp {
    let mut result = ClientsIp {
        bytes: [0; 16],
        length: 0,
    };
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
/// Whether the leading 4 (version 4) or 16 (version 6) address bytes are a
/// loopback address. Original IPv4 policy recognizes only 127.0.0.1, not /8.
#[must_use]
pub fn swr_v1_clients_loopback(version: u8, address: &[u8]) -> u8 {
    let length = match version {
        4 => 4,
        6 => 16,
        _ => return 0,
    };
    let Some(bytes) = address.get(..length) else {
        return 0;
    };
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
/// The leading 4 (version 4) or 16 (version 6) address bytes as text. DNS IPv6
/// output deliberately keeps all eight four-digit groups.
#[must_use]
pub fn swr_v1_clients_ip_text(version: u8, address: &[u8]) -> ClientsIpText {
    let mut output = ClientsIpText {
        bytes: [0; 40],
        length: 0,
    };
    let mut text = [0u8; 40];
    let written = match version {
        4 => {
            let Some(&v4) = address.first_chunk::<4>() else {
                return output;
            };
            crate::format_into(&mut text, format_args!("{}", Ipv4Addr::from(v4)))
        }
        6 => {
            let Some(&v6) = address.first_chunk::<16>() else {
                return output;
            };
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

#[cfg(test)]
mod tests {
    use super::{swr_v1_clients_ip, swr_v1_clients_ip_text, swr_v1_clients_loopback};

    #[test]
    fn short_address_slices_are_not_addresses() {
        let loopback6 = std::net::Ipv6Addr::LOCALHOST.octets();
        for (version, full) in [(4, &[127, 0, 0, 1][..]), (6, &loopback6[..])] {
            assert_eq!(swr_v1_clients_loopback(version, full), 1);
            assert_ne!(swr_v1_clients_ip_text(version, full).length, 0);
            for short in [&[][..], &full[..full.len() - 1]] {
                assert_eq!(swr_v1_clients_loopback(version, short), 0);
                assert_eq!(swr_v1_clients_ip_text(version, short).length, 0);
            }
        }
        assert_eq!(swr_v1_clients_ip(4, b"", 0).length, 0);
        assert_eq!(swr_v1_clients_ip(6, b"", 0).length, 0);
    }
}
