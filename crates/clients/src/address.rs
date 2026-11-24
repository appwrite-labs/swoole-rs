//! `network::Address`'s operations for native code. `assign` is crates/corebase's,
//! with a host that is not an IP literal going to the platform or, when allowed,
//! native `gethostbyname`. Native code keeps `type`, set before the call, and the
//! last error.
use crate::ip::swr_v1_clients_ip;
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::constants::SocketType;
use swoole_rs_corebase::net_address::{BAD_HOST_ADDR, DNSLOOKUP_RESOLVE_FAILED, platform_parses};
use swoole_rs_corebase::net_types::{AddressStorage, SocketAddress};
use swoole_rs_corebase::ntop;
use swoole_rs_corebase::sys::Text;

/// The host and port of an address that holds its socket type's member, as
/// upstream's warnings print them.
#[must_use]
pub fn host_port(address: &SocketAddress) -> (String, c_int) {
    let host = with_host(address, |host| host.to_string_lossy().into_owned());
    (host, address_port(address).into())
}

/// `clients_parse_ip`: the host as an IP literal of `family` into `destination`,
/// which holds any family's address.
fn parse_ip(family: c_int, host: &CStr, destination: &mut [u8; 16]) -> bool {
    let text = host.to_bytes();
    // Other families, and the hosts crates/corebase leaves to it, go to the platform.
    if (family != libc::AF_INET && family != libc::AF_INET6) || platform_parses(family, text) {
        return swoole_rs_corebase::pton(family, host, destination);
    }
    let version = if family == libc::AF_INET { 4 } else { 6 };
    let allow_zeroes = u8::from(cfg!(target_os = "macos"));
    let ip = swr_v1_clients_ip(version, text, allow_zeroes);
    let length = usize::try_from(ip.length).unwrap_or(0);
    match (destination.get_mut(..length), ip.bytes.get(..length)) {
        (Some(to), Some(from)) if length != 0 => {
            to.copy_from_slice(from);
            true
        }
        _ => false,
    }
}

/// [`assign`] with `resolve` for a host that is no IP literal.
///
/// # Errors
/// The `SW_ERROR_*` code.
pub fn assign_with(
    address: &mut AddressStorage,
    length: &mut libc::socklen_t,
    kind: c_int,
    host: &[u8],
    port: c_int,
    resolve: impl FnOnce(c_int, &CStr) -> Result<[u8; 16], c_int>,
) -> Result<(), c_int> {
    let Some((&0, bytes)) = host.split_last() else {
        return Err(BAD_HOST_ADDR);
    };
    // Only a host that is no IP literal of the family needs the name.
    swoole_rs_corebase::net_address::assign(address, length, kind, bytes, port, |family| {
        let Ok(name) = CStr::from_bytes_until_nul(host) else {
            return Err(BAD_HOST_ADDR);
        };
        let mut address = [0; 16];
        if parse_ip(family, name, &mut address) {
            return Ok(address);
        }
        resolve(family, name)
    })
}

/// `Address::assign(kind, host, port, resolve)`: fills `address` and `*length`.
/// `host` is the host's bytes and the NUL that ends them (`std::string::c_str()`):
/// a resolver reads it up to its first NUL. `resolve` looks up a host that is no IP
/// literal with `gethostbyname`.
///
/// # Errors
/// The `SW_ERROR_*` code for the caller to set as the last error.
pub fn assign(
    address: &mut AddressStorage,
    length: &mut libc::socklen_t,
    kind: c_int,
    host: &[u8],
    port: c_int,
    resolve: bool,
) -> Result<(), c_int> {
    assign_with(address, length, kind, host, port, |family, name| {
        if !resolve {
            return Err(BAD_HOST_ADDR);
        }
        let binary = swoole_rs_corebase::gethostbyname(family, name);
        let binary = binary.ok_or(DNSLOOKUP_RESOLVE_FAILED)?;
        let mut address = [0; 16];
        for (byte, c) in address.iter_mut().zip(binary) {
            *byte = c.to_ne_bytes()[0];
        }
        Ok(address)
    })
}

// Address::addr_str and verify_ip deliberately share one per-thread buffer.
// Its pointer stays valid until thread exit and each operation overwrites it.
thread_local! {
    static ADDRESS_SCRATCH: Cell<[u8; 46]> = const { Cell::new([0; 46]) };
}

fn inet4(kind: c_int) -> bool {
    SocketType::from_raw(kind).is_some_and(SocketType::is_inet4)
}

fn inet6(kind: c_int) -> bool {
    SocketType::from_raw(kind).is_some_and(SocketType::is_inet6)
}

fn local(kind: c_int) -> bool {
    SocketType::from_raw(kind).is_some_and(SocketType::is_local)
}

/// The bytes of an address of `family`: 4 for IPv4, 16 for IPv6, none for another.
fn address_size(family: c_int) -> usize {
    match family {
        libc::AF_INET => 4,
        libc::AF_INET6 => 16,
        _ => 0,
    }
}

/// `Address::addr_str`: `address` (of `family`) as text in this thread's scratch,
/// which the next call overwrites; null when it does not format or `address` is too
/// short for the family.
#[must_use]
pub fn address_string(family: c_int, address: &[u8]) -> *const c_char {
    let Some(given) = address.get(..address_size(family)) else {
        return std::ptr::null();
    };
    // The family's bytes, in room for either's.
    let mut bytes = [0; 16];
    for (byte, &given) in bytes.iter_mut().zip(given) {
        *byte = c_char::from_ne_bytes([given]);
    }
    let Some(text) = ntop(family, &bytes) else {
        return std::ptr::null();
    };
    ADDRESS_SCRATCH.with(|scratch| {
        scratch.set(text.map(|unit| unit.to_ne_bytes()[0]));
        scratch.as_ptr().cast()
    })
}

/// `Address::verify_ip` for Rust: whether `host` is an IP literal of `family`. The
/// address goes to this call's own buffer rather than the thread's scratch.
#[must_use]
pub fn verify_ip(family: c_int, host: &CStr) -> bool {
    parse_ip(family, host, &mut [0; 16])
}

/// `Address::verify_ip`, writing the same scratch as `addr_str` does.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_verify(family: c_int, host: Text<'_>) -> bool {
    let host = host.get();
    ADDRESS_SCRATCH.with(|scratch| {
        // The scratch's leading bytes, which hold the largest IP address, parsed
        // into in a copy and stored back.
        let mut text = scratch.get();
        let parsed = text
            .first_chunk_mut()
            .is_some_and(|destination| parse_ip(family, host, destination));
        scratch.set(text);
        parsed
    })
}

/// `Address::get_addr`: IP text in shared scratch, Unix path in the record, or
/// the original static fallback for an unknown socket type. The Unix path
/// remains valid only while the record does; IP text lives until reused.
#[must_use]
pub fn host_in_scratch(address: &SocketAddress) -> *const c_char {
    if inet4(address.kind) {
        let ip = address.addr.v4().sin_addr.s_addr;
        address_string(libc::AF_INET, &ip.to_ne_bytes())
    } else if inet6(address.kind) {
        let ip = address.addr.v6().sin6_addr.s6_addr;
        address_string(libc::AF_INET6, &ip)
    } else if local(address.kind) {
        // A local socket selects the initialized Unix union member.
        (&raw const address.addr.un)
            .wrapping_byte_add(std::mem::offset_of!(libc::sockaddr_un, sun_path))
            .cast()
    } else {
        c"unknown".as_ptr()
    }
}

/// The first `length` bytes (at most 128) where [`host_in_scratch`] puts `address`'s
/// text, zeros past their end, as upstream's `crc32(get_addr(), len)` hashes them: the
/// IP text in this thread's scratch, the Unix path and the storage after it, or the
/// fallback. `None` when the IP does not format.
#[must_use]
pub fn host_bytes(address: &SocketAddress, length: usize) -> Option<[u8; 128]> {
    let mut bytes = [0; 128];
    let mut copy = |text: &[u8]| {
        let length = length.min(bytes.len());
        bytes
            .iter_mut()
            .take(length)
            .zip(text)
            .for_each(|(to, from)| *to = *from);
    };
    if inet4(address.kind) || inet6(address.kind) {
        if host_in_scratch(address).is_null() {
            return None;
        }
        copy(&ADDRESS_SCRATCH.with(Cell::get));
    } else if local(address.kind) {
        let path = std::mem::offset_of!(libc::sockaddr_un, sun_path);
        copy(address.addr.as_bytes().get(path..).unwrap_or_default());
    } else {
        copy(c"unknown".to_bytes_with_nul());
    }
    Some(bytes)
}

/// `Address::get_addr` for Rust: `f` reads the text, which an IP address formats
/// into this call's own buffer rather than the thread's scratch, so nothing can
/// overwrite it while `f` runs.
pub fn with_host<R>(address: &SocketAddress, f: impl FnOnce(&CStr) -> R) -> R {
    address.with_host(f)
}

/// `Address::get_port`: 0 for an address that is not IP.
#[must_use]
pub fn address_port(address: &SocketAddress) -> u16 {
    address.port()
}

/// `Address::set_port`, including the original int-to-uint16_t wrapping.
pub fn set_port(address: &mut SocketAddress, port: c_int) {
    let port = port.c_cast::<u16>().to_be();
    if inet4(address.kind) {
        address.addr.inet_v4.sin_port = port;
    } else if inet6(address.kind) {
        address.addr.inet_v6.sin6_port = port;
    }
}

/// `Address::empty`.
#[must_use]
pub fn is_empty(address: &SocketAddress) -> bool {
    address.kind == 0
}

/// `Address::type_str`'s original static spelling and family classification.
#[must_use]
pub fn type_name(kind: c_int) -> &'static CStr {
    if inet4(kind) {
        c"IPv4"
    } else if inet6(kind) {
        c"IPv6"
    } else if local(kind) {
        c"UnixSocket"
    } else {
        c"Unknown"
    }
}

/// `Address::is_loopback_addr`, preserving exact IPv4 localhost semantics.
#[must_use]
pub fn is_loopback(address: &SocketAddress) -> bool {
    let loopback = crate::ip::swr_v1_clients_loopback;
    if inet4(address.kind) {
        let ip = address.addr.v4().sin_addr.s_addr;
        loopback(4, &ip.to_ne_bytes()) != 0
    } else if inet6(address.kind) {
        let ip = address.addr.v6().sin6_addr.s6_addr;
        loopback(6, &ip) != 0
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_corebase::net_address::{BAD_PORT, BAD_SOCKET_TYPE, NAME_TOO_LONG};

    const TCP: c_int = SocketType::SW_SOCK_TCP.raw();
    const TCP6: c_int = SocketType::SW_SOCK_TCP6.raw();
    const UDP: c_int = SocketType::SW_SOCK_UDP.raw();
    const UDP6: c_int = SocketType::SW_SOCK_UDP6.raw();
    const UNIX_STREAM: c_int = SocketType::SW_SOCK_UNIX_STREAM.raw();
    const UNIX_DGRAM: c_int = SocketType::SW_SOCK_UNIX_DGRAM.raw();

    #[repr(C)]
    union Union {
        v4: libc::sockaddr_in,
        v6: libc::sockaddr_in6,
        un: libc::sockaddr_un,
    }

    /// A `gethostbyname` double that finds an address of the family, or fails.
    fn lookup(family: c_int, found: bool) -> Result<[u8; 16], c_int> {
        let bytes: &[u8] = if family == libc::AF_INET {
            &[10, 0, 0, 7]
        } else {
            &[0xfe, 0x80]
        };
        let mut address = [0; 16];
        address[..bytes.len()].copy_from_slice(bytes);
        found.then_some(address).ok_or(DNSLOOKUP_RESOLVE_FAILED)
    }
    const RESOLVES: Option<bool> = Some(true);
    const FAILS: Option<bool> = Some(false);

    fn assign(
        kind: c_int,
        host: &std::ffi::CStr,
        port: c_int,
        resolve: Option<bool>,
    ) -> (c_int, Union, libc::socklen_t) {
        let mut address: Union = unsafe { std::mem::zeroed() };
        let mut length = 0;
        let storage = unsafe { &mut *(&raw mut address).cast::<AddressStorage>() };
        let host = host.to_bytes_with_nul();
        let assigned = assign_with(storage, &mut length, kind, host, port, |family, _| {
            resolve.map_or(Err(BAD_HOST_ADDR), |found| lookup(family, found))
        });
        (assigned.err().unwrap_or(0), address, length)
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_pton")]
    fn fills_each_family() {
        let (error, address, length) = assign(TCP, c"127.0.0.1", 8080, None);
        assert_eq!(
            (error, length as usize),
            (0, size_of::<libc::sockaddr_in>())
        );
        let v4 = unsafe { address.v4 };
        assert_eq!(
            (u16::from_be(v4.sin_port), v4.sin_addr.s_addr.to_ne_bytes()),
            (8080, [127, 0, 0, 1])
        );

        let (error, address, length) = assign(UDP6, c"::1", 53, None);
        assert_eq!(
            (error, length as usize),
            (0, size_of::<libc::sockaddr_in6>())
        );
        assert_eq!(unsafe { address.v6 }.sin6_addr.s6_addr[15], 1);

        let (error, address, _) = assign(TCP, c"example.test", 1, RESOLVES);
        assert_eq!(
            (error, unsafe { address.v4 }.sin_addr.s_addr.to_ne_bytes()),
            (0, [10, 0, 0, 7])
        );
        assert_eq!(assign(TCP, c"example.test", 1, None).0, BAD_HOST_ADDR);
        assert_eq!(
            assign(TCP6, c"example.test", 1, FAILS).0,
            DNSLOOKUP_RESOLVE_FAILED
        );

        let (error, address, length) = assign(UNIX_STREAM, c"/tmp/s.sock", 0, None);
        let un = unsafe { address.un };
        assert_eq!((error, length as usize), (0, un.sun_path.len()));
        assert_eq!(
            un.sun_path[..12]
                .iter()
                .map(|&c| c.to_ne_bytes()[0])
                .collect::<Vec<_>>(),
            b"/tmp/s.sock\0"
        );
        let long = std::ffi::CString::new("x".repeat(un.sun_path.len() - 1)).unwrap();
        assert_eq!(assign(UNIX_DGRAM, &long, 0, None).0, NAME_TOO_LONG);

        assert_eq!(assign(TCP, c"127.0.0.1", 70000, None).0, BAD_PORT);
        assert_eq!(assign(9, c"127.0.0.1", 1, None).0, BAD_SOCKET_TYPE);
        for (family, host, valid) in [
            (libc::AF_INET, c"10.1.2.3", true),
            (libc::AF_INET, c"::1", false),
            (libc::AF_INET6, c"fe80::1", true),
            (libc::AF_UNIX, c"10.1.2.3", false),
        ] {
            assert_eq!(swr_v1_clients_address_verify(family, host.into()), valid);
        }
    }

    fn socket_address(kind: c_int, host: &CStr) -> SocketAddress {
        let mut address = SocketAddress {
            kind,
            ..SocketAddress::default()
        };
        let (storage, length) = (&mut address.addr, &mut address.len);
        let assigned = super::assign(storage, length, kind, host.to_bytes_with_nul(), 80, false);
        assert_eq!(assigned, Ok(()), "{host:?}");
        address
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_pton or inet_ntop")]
    fn with_host_formats_as_the_scratch_does() {
        for (kind, host) in [
            (TCP, c"10.1.2.3"),
            (UDP, c"127.0.0.1"),
            (TCP6, c"fe80::1"),
            (UDP6, c"::ffff:1.2.3.4"),
            (UNIX_STREAM, c"/tmp/s.sock"),
            (UNIX_DGRAM, c""),
        ] {
            let address = socket_address(kind, host);
            assert_eq!(with_host(&address, CStr::to_owned).as_c_str(), host);
            // SAFETY: the scratch text, read before this thread formats another.
            let scratch = unsafe { CStr::from_ptr(host_in_scratch(&address)) };
            assert_eq!(scratch, host, "the scratch agrees");
        }
        let unknown = SocketAddress {
            kind: 9,
            ..SocketAddress::default()
        };
        assert_eq!(with_host(&unknown, CStr::to_owned).as_c_str(), c"unknown");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_pton or inet_ntop")]
    fn host_bytes_are_the_texts_bytes_then_zeros() {
        let v6 = socket_address(UDP6, c"fe80::1");
        let bytes = host_bytes(&v6, 28).unwrap();
        assert_eq!(&bytes[..8], b"fe80::1\0");
        assert!(bytes[8..].iter().all(|&byte| byte == 0));
        let unix = socket_address(UNIX_DGRAM, c"/tmp/s.sock");
        let bytes = host_bytes(&unix, 14).unwrap();
        assert_eq!(&bytes[..12], b"/tmp/s.sock\0");
        assert!(
            bytes[14..].iter().all(|&byte| byte == 0),
            "only `length` bytes"
        );
        let unknown = SocketAddress {
            kind: 9,
            ..SocketAddress::default()
        };
        assert_eq!(&host_bytes(&unknown, 10).unwrap()[..10], b"unknown\0\0\0");
    }

    #[test]
    fn with_host_ends_an_unterminated_unix_path() {
        let mut address = SocketAddress {
            kind: UNIX_STREAM,
            ..SocketAddress::default()
        };
        let len = address.addr.edit_unix(|un| {
            un.sun_path.fill(c_char::from_ne_bytes(*b"x"));
            un.sun_path.len()
        });
        let full = vec![b'x'; len];
        assert!(with_host(&address, |host| host.to_bytes() == full));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_pton or inet_ntop")]
    fn with_host_text_outlives_other_formatting() {
        let v4 = socket_address(TCP, c"10.1.2.3");
        let v6 = socket_address(TCP6, c"fe80::1");
        with_host(&v4, |outer| {
            // Neither another lend nor the shared scratch touches `outer`.
            assert_eq!(with_host(&v6, CStr::to_owned).as_c_str(), c"fe80::1");
            assert!(!host_in_scratch(&v6).is_null());
            assert_eq!(outer, c"10.1.2.3");
        });
    }
}
