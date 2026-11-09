//! `network::Address::assign(type, host, port, resolve_name)` over an
//! [`AddressStorage`]: the sockaddr and length for an IPv4, IPv6 or Unix socket type,
//! the host parsed as an IP literal (with the numeric grammar native `inet_pton`
//! accepts) or handed to the caller's fallback. crates/clients exports it to native
//! code with the resolver; crates/tls binds with it.
use crate::net_types::AddressStorage;
use std::ffi::c_int;
use std::net::{Ipv4Addr, Ipv6Addr};

/// `SocketType` values from `swoole.h`.
pub const TCP: c_int = 1;
pub const UDP: c_int = 2;
pub const TCP6: c_int = 3;
pub const UDP6: c_int = 4;
pub const UNIX_STREAM: c_int = 5;
pub const UNIX_DGRAM: c_int = 6;
pub const RAW: c_int = 7;
pub const RAW6: c_int = 8;

/// `SW_ERROR_*` codes `assign` returns.
pub const NAME_TOO_LONG: c_int = 504;
pub const DNSLOOKUP_RESOLVE_FAILED: c_int = 711;
pub const BAD_HOST_ADDR: c_int = 722;
pub const BAD_PORT: c_int = 723;
pub const BAD_SOCKET_TYPE: c_int = 724;

/// `sockaddr_un::sun_path`'s length.
const SUN_PATH: usize =
    size_of::<libc::sockaddr_un>() - core::mem::offset_of!(libc::sockaddr_un, sun_path);

/// The bytes before the first NUL, as a C string reads them.
#[must_use]
pub fn c_str(bytes: &[u8]) -> &[u8] {
    bytes.split(|&byte| byte == 0).next().unwrap_or_default()
}

/// Formats `args` into `buffer` without allocating; `None` when it does not fit.
pub fn format_into<'a>(buffer: &'a mut [u8], args: std::fmt::Arguments<'_>) -> Option<&'a [u8]> {
    let mut rest = &mut *buffer;
    std::io::Write::write_fmt(&mut rest, args).ok()?;
    let unused = rest.len();
    buffer.get(..buffer.len() - unused)
}

#[must_use]
pub fn ipv4(text: &str, allow_zeroes: bool) -> Option<Ipv4Addr> {
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

#[must_use]
pub fn ipv6(text: &str, allow_zeroes: bool) -> Option<Ipv6Addr> {
    if !text.contains('.') {
        return text.parse().ok();
    }
    let colon = text.rfind(':')?;
    let [a, b, c, d] = ipv4(text.get(colon + 1..)?, allow_zeroes)?.octets();
    // Replace the dotted tail with two hex groups in bounded caller-local
    // storage. This preserves native Apple decimal-leading-zero grammar without
    // introducing an allocation effect into numeric parsing.
    let mut normalized = [0u8; 128];
    let normalized = format_into(
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

/// Whether `host` is left to the platform's `inet_pton` rather than parsed here:
/// Apple's IPv6 scope names, which it resolves as it parses (changing errno).
#[must_use]
pub fn platform_parses(family: c_int, host: &[u8]) -> bool {
    cfg!(target_os = "macos") && family == libc::AF_INET6 && host.contains(&b'%')
}

/// `host` (the bytes before any NUL) as an IP literal of `family`, unless the
/// platform parses it.
fn literal(family: c_int, host: &[u8]) -> Option<[u8; 16]> {
    if platform_parses(family, host) {
        return None;
    }
    let text = std::str::from_utf8(host).ok()?;
    let allow_zeroes = cfg!(target_os = "macos");
    let mut bytes = [0; 16];
    if family == libc::AF_INET {
        bytes[..4].copy_from_slice(&ipv4(text, allow_zeroes)?.octets());
    } else {
        bytes = ipv6(text, allow_zeroes)?.octets();
    }
    Some(bytes)
}

/// Fills `address` and `*length` for `kind`, `host` (its first `host.len()` bytes,
/// read up to any NUL) and `port`; 0, or the `SW_ERROR_*` code to set as the last
/// error. A host that is not an IP literal of the family goes to `fallback`, which
/// returns its address bytes or the error.
pub fn assign(
    address: &mut AddressStorage,
    length: &mut libc::socklen_t,
    kind: c_int,
    host: &[u8],
    port: c_int,
    fallback: impl FnOnce(c_int) -> Result<[u8; 16], c_int>,
) -> c_int {
    let Ok(port) = u16::try_from(port) else {
        return BAD_PORT;
    };
    let port = port.to_be();
    let size = |of: usize| libc::socklen_t::try_from(of).unwrap_or(0);
    let text = c_str(host);
    let resolved = |family| literal(family, text).map_or_else(|| fallback(family), Ok);
    match kind {
        TCP | UDP | RAW => {
            address.inet_v4.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap_or(0);
            address.inet_v4.sin_port = port;
            *length = size(size_of::<libc::sockaddr_in>());
            let [a, b, c, d, ..] = match resolved(libc::AF_INET) {
                Ok(bytes) => bytes,
                Err(error) => return error,
            };
            address.inet_v4.sin_addr.s_addr = u32::from_ne_bytes([a, b, c, d]);
            0
        }
        TCP6 | UDP6 | RAW6 => {
            address.inet_v6.sin6_family = libc::sa_family_t::try_from(libc::AF_INET6).unwrap_or(0);
            address.inet_v6.sin6_port = port;
            *length = size(size_of::<libc::sockaddr_in6>());
            match resolved(libc::AF_INET6) {
                Ok(bytes) => address.inet_v6.sin6_addr.s6_addr = bytes,
                Err(error) => return error,
            }
            0
        }
        UNIX_STREAM | UNIX_DGRAM => {
            if host.len() >= SUN_PATH - 1 {
                return NAME_TOO_LONG;
            }
            address.un.sun_family = libc::sa_family_t::try_from(libc::AF_UNIX).unwrap_or(0);
            // SAFETY: the Unix member's path is plain bytes, valid whatever the union
            // last held.
            let path = unsafe { &mut address.un.sun_path };
            // `swoole_strlcpy` and the forced final NUL; the bytes after are kept.
            for (slot, byte) in path.iter_mut().zip(text.iter().copied().chain([0])) {
                *slot = libc::c_char::from_ne_bytes([byte]);
            }
            *path.last_mut().unwrap_or(&mut 0) = 0;
            *length = size(SUN_PATH);
            0
        }
        _ => BAD_SOCKET_TYPE,
    }
}
