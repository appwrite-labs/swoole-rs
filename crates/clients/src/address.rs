//! `network::Address`'s operations for native code. `assign` is crates/corebase's,
//! with a host that is not an IP literal going to the platform or, when allowed,
//! native `gethostbyname`. Native code keeps `type`, set before the call, and the
//! last error.
use crate::ip::swr_v1_clients_ip;
use std::cell::UnsafeCell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::Write;
use std::net::Ipv4Addr;
use swoole_rs_corebase::net_address::{
    BAD_HOST_ADDR, DNSLOOKUP_RESOLVE_FAILED, RAW, RAW6, TCP, TCP6, UDP, UDP6, UNIX_DGRAM,
    UNIX_STREAM, platform_parses,
};
use swoole_rs_corebase::net_types::{AddressStorage, SocketAddress};
use swoole_rs_corebase::ntop;
use swoole_rs_tls::CText;

// SAFETY: libc's own declarations. `inet_pton` reads the NUL-terminated source and
// writes at most an IPv6 address's 16 bytes (nothing for a family it doesn't know).
unsafe extern "C" {
    fn inet_ntop(
        family: c_int,
        address: *const c_void,
        text: *mut c_char,
        size: libc::socklen_t,
    ) -> *const c_char;
    safe fn inet_pton(family: c_int, source: CText<'_>, destination: &mut [u8; 16]) -> c_int;
}

/// `network::gethostbyname(family, name, address)`: writes the address, < 0 on failure.
pub type Resolve = unsafe extern "C" fn(c_int, *const c_char, *mut c_char) -> c_int;

/// `clients_parse_ip`: the host as an IP literal of `family` into `destination`,
/// which holds any family's address.
fn parse_ip(family: c_int, host: &CStr, destination: &mut [u8; 16]) -> bool {
    let text = host.to_bytes();
    // Other families, and the hosts crates/corebase leaves to it, go to the platform.
    if (family != libc::AF_INET && family != libc::AF_INET6) || platform_parses(family, text) {
        return inet_pton(family, host.into(), destination) == 1;
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

/// [`assign`] with `resolve` for a host that is no IP literal: the address, or the
/// `SW_ERROR_*` code.
fn assign_with(
    address: &mut AddressStorage,
    length: &mut libc::socklen_t,
    kind: c_int,
    host: &[u8],
    port: c_int,
    resolve: impl FnOnce(c_int, &CStr) -> Result<[u8; 16], c_int>,
) -> c_int {
    let Some((&0, bytes)) = host.split_last() else {
        return BAD_HOST_ADDR;
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

/// `Address::assign(kind, host, port, resolve)`: fills `address` and `*length`;
/// 0, or the `SW_ERROR_*` code for the caller to set as the last error. `host` is
/// the host's bytes and the NUL that ends them (`std::string::c_str()`): a resolver
/// reads it up to its first NUL. `resolve` looks up a host that is no IP literal
/// with `gethostbyname`.
pub fn assign(
    address: &mut AddressStorage,
    length: &mut libc::socklen_t,
    kind: c_int,
    host: &[u8],
    port: c_int,
    resolve: bool,
) -> c_int {
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

/// [`assign`] for native code, whose `resolve` is native `gethostbyname` or null.
///
/// # Safety
/// `address` points to the union, `length` to its `socklen_t`, `host` to a
/// NUL-terminated string of `host_length` bytes before that NUL; `resolve` is
/// native `gethostbyname` or null (`resolve_name` false).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_address_assign(
    address: *mut c_void,
    length: *mut libc::socklen_t,
    kind: c_int,
    host: *const c_char,
    host_length: usize,
    port: c_int,
    resolve: Option<Resolve>,
) -> c_int {
    // SAFETY: the caller passes the union.
    let storage = unsafe { &mut *address.cast::<AddressStorage>() };
    // SAFETY: and its length.
    let size = unsafe { &mut *length };
    // SAFETY: and `host_length` bytes of host, then its NUL.
    let bytes =
        unsafe { std::slice::from_raw_parts(host.cast::<u8>(), host_length.saturating_add(1)) };
    assign_with(storage, size, kind, bytes, port, |family, name| {
        let Some(resolve) = resolve else {
            return Err(BAD_HOST_ADDR);
        };
        let mut address = [0u8; 16];
        // SAFETY: native `gethostbyname`, a terminated name and room for either
        // family's address.
        if unsafe { resolve(family, name.as_ptr(), address.as_mut_ptr().cast()) } < 0 {
            return Err(DNSLOOKUP_RESOLVE_FAILED);
        }
        Ok(address)
    })
}

// Address::addr_str and verify_ip deliberately share one per-thread buffer.
// Its pointer stays valid until thread exit and each operation overwrites it.
thread_local! {
    static ADDRESS_SCRATCH: UnsafeCell<[c_char; 46]> = const { UnsafeCell::new([0; 46]) };
}

fn inet4(kind: c_int) -> bool {
    matches!(kind, TCP | UDP | RAW)
}

fn inet6(kind: c_int) -> bool {
    matches!(kind, TCP6 | UDP6 | RAW6)
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
    if address.len() < address_size(family) {
        return std::ptr::null();
    }
    ADDRESS_SCRATCH.with(|scratch| {
        // SAFETY: this thread alone accesses its fixed scratch; inet_ntop reads the
        // family's bytes, writes at most its supplied capacity and retains neither.
        unsafe { inet_ntop(family, address.as_ptr().cast(), scratch.get().cast(), 46) }
    })
}

/// [`address_string`] for native code.
/// # Safety
/// `address` points to the address bytes for `family`, as required by `inet_ntop`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_address_string(
    family: c_int,
    address: *const c_void,
) -> *const c_char {
    // SAFETY: the family's address bytes, per the caller.
    let address = unsafe { std::slice::from_raw_parts(address.cast(), address_size(family)) };
    address_string(family, address)
}

/// `Address::verify_ip` for Rust: whether `host` is an IP literal of `family`. The
/// address goes to this call's own buffer rather than the thread's scratch.
#[must_use]
pub fn verify_ip(family: c_int, host: &CStr) -> bool {
    parse_ip(family, host, &mut [0; 16])
}

/// `Address::verify_ip`, writing the same scratch as `addr_str` does.
/// # Safety
/// `host` is a live NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_address_verify(family: c_int, host: *const c_char) -> bool {
    // SAFETY: the caller's NUL-terminated host.
    let host = unsafe { CStr::from_ptr(host) };
    ADDRESS_SCRATCH.with(|scratch| {
        // SAFETY: the scratch holds the largest IP address; this thread alone
        // accesses it, and no borrow of it outlives this call.
        let destination = unsafe { &mut *scratch.get().cast::<[u8; 16]>() };
        parse_ip(family, host, destination)
    })
}

/// `Address::get_addr`: IP text in shared scratch, Unix path in the record, or
/// the original static fallback for an unknown socket type. The Unix path
/// remains valid only while the record does; IP text lives until reused.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_host(address: &SocketAddress) -> *const c_char {
    if inet4(address.kind) {
        let ip = address.addr.v4().sin_addr.s_addr;
        address_string(libc::AF_INET, &ip.to_ne_bytes())
    } else if inet6(address.kind) {
        let ip = address.addr.v6().sin6_addr.s6_addr;
        address_string(libc::AF_INET6, &ip)
    } else if matches!(address.kind, UNIX_STREAM | UNIX_DGRAM) {
        // A local socket selects the initialized Unix union member.
        (&raw const address.addr.un)
            .wrapping_byte_add(std::mem::offset_of!(libc::sockaddr_un, sun_path))
            .cast()
    } else {
        c"unknown".as_ptr()
    }
}

/// `Address::get_addr` for Rust: `f` reads the text, which an IP address formats
/// into this call's own buffer rather than the thread's scratch, so nothing can
/// overwrite it while `f` runs.
pub fn with_host<R>(address: &SocketAddress, f: impl FnOnce(&CStr) -> R) -> R {
    if inet4(address.kind) {
        // `inet_ntop(AF_INET)` writes the four octets in decimal, as `Ipv4Addr` displays;
        // at most 15 characters, which leave the buffer its terminator.
        let mut text = [0u8; 46];
        let ip = Ipv4Addr::from(address.addr.v4().sin_addr.s_addr.to_ne_bytes());
        if write!(&mut text[..45], "{ip}").is_err() {
            return f(c"");
        }
        f(CStr::from_bytes_until_nul(&text).unwrap_or_default())
    } else if inet6(address.kind) {
        let ip = address.addr.v6().sin6_addr.s6_addr;
        // `inet_ntop(AF_INET6)`'s text, empty when it fails.
        let text = ntop(
            libc::AF_INET6,
            &ip.map(|byte| c_char::from_ne_bytes([byte])),
        );
        let text = text.unwrap_or([0; 46]).map(|unit| unit.to_ne_bytes()[0]);
        f(CStr::from_bytes_until_nul(&text).unwrap_or_default())
    } else if matches!(address.kind, UNIX_STREAM | UNIX_DGRAM) {
        let path = address.addr.unix().sun_path;
        // A byte past the path, so an unterminated one still ends.
        let mut bytes = [0u8; 109];
        for (byte, c) in bytes.iter_mut().zip(path) {
            *byte = c.to_ne_bytes()[0];
        }
        f(CStr::from_bytes_until_nul(&bytes).unwrap_or_default())
    } else {
        f(c"unknown")
    }
}

/// `Address::get_port`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_port(address: &SocketAddress) -> c_int {
    let port = if inet4(address.kind) {
        address.addr.v4().sin_port
    } else if inet6(address.kind) {
        address.addr.v6().sin6_port
    } else {
        return 0;
    };
    c_int::from(u16::from_be(port))
}

/// `Address::set_port`, including the original int-to-uint16_t wrapping.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "htons accepts the low 16 bits of upstream's int argument"
)]
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_set_port(address: &mut SocketAddress, port: c_int) {
    let port = (port as u16).to_be();
    if inet4(address.kind) {
        address.addr.inet_v4.sin_port = port;
    } else if inet6(address.kind) {
        address.addr.inet_v6.sin6_port = port;
    }
}

/// `Address::empty`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_empty(address: &SocketAddress) -> bool {
    address.kind == 0
}

/// `Address::type_str`'s original static spelling and family classification.
#[must_use]
pub fn type_name(kind: c_int) -> &'static CStr {
    if inet4(kind) {
        c"IPv4"
    } else if inet6(kind) {
        c"IPv6"
    } else if matches!(kind, UNIX_STREAM | UNIX_DGRAM) {
        c"UnixSocket"
    } else {
        c"Unknown"
    }
}

/// [`type_name`] for native code.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_type(kind: c_int) -> *const c_char {
    type_name(kind).as_ptr()
}

/// `Address::is_loopback_addr`, preserving exact IPv4 localhost semantics.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_loopback(address: &SocketAddress) -> bool {
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

    #[repr(C)]
    union Union {
        v4: libc::sockaddr_in,
        v6: libc::sockaddr_in6,
        un: libc::sockaddr_un,
    }

    unsafe extern "C" fn resolves(family: c_int, _: *const c_char, out: *mut c_char) -> c_int {
        let bytes: &[u8] = if family == libc::AF_INET {
            &[10, 0, 0, 7]
        } else {
            &[0xfe, 0x80]
        };
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast(), bytes.len()) };
        0
    }
    unsafe extern "C" fn fails(_: c_int, _: *const c_char, _: *mut c_char) -> c_int {
        -1
    }

    fn assign(
        kind: c_int,
        host: &std::ffi::CStr,
        port: c_int,
        resolve: Option<Resolve>,
    ) -> (c_int, Union, libc::socklen_t) {
        let mut address: Union = unsafe { std::mem::zeroed() };
        let mut length = 0;
        let error = unsafe {
            swr_v1_clients_address_assign(
                (&raw mut address).cast(),
                &raw mut length,
                kind,
                host.as_ptr(),
                host.to_bytes().len(),
                port,
                resolve,
            )
        };
        (error, address, length)
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

        let (error, address, _) = assign(TCP, c"example.test", 1, Some(resolves));
        assert_eq!(
            (error, unsafe { address.v4 }.sin_addr.s_addr.to_ne_bytes()),
            (0, [10, 0, 0, 7])
        );
        assert_eq!(assign(TCP, c"example.test", 1, None).0, BAD_HOST_ADDR);
        assert_eq!(
            assign(TCP6, c"example.test", 1, Some(fails)).0,
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
            assert_eq!(
                unsafe { swr_v1_clients_address_verify(family, host.as_ptr()) },
                valid
            );
        }
    }

    fn socket_address(kind: c_int, host: &CStr) -> SocketAddress {
        let mut address = SocketAddress {
            kind,
            ..SocketAddress::default()
        };
        let error = unsafe {
            swr_v1_clients_address_assign(
                (&raw mut address.addr).cast(),
                &raw mut address.len,
                kind,
                host.as_ptr(),
                host.to_bytes().len(),
                80,
                None,
            )
        };
        assert_eq!(error, 0, "{host:?}");
        address
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_pton or inet_ntop")]
    fn with_host_formats_as_the_export_does() {
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
            // SAFETY: the export's text, read before this thread formats another.
            let export = unsafe { CStr::from_ptr(swr_v1_clients_address_host(&address)) };
            assert_eq!(export, host, "the export agrees");
        }
        let unknown = SocketAddress {
            kind: 9,
            ..SocketAddress::default()
        };
        assert_eq!(with_host(&unknown, CStr::to_owned).as_c_str(), c"unknown");
    }

    #[test]
    fn with_host_ends_an_unterminated_unix_path() {
        let mut address = SocketAddress {
            kind: UNIX_STREAM,
            ..SocketAddress::default()
        };
        let path = &mut address.addr.unix_mut().sun_path;
        path.fill(c_char::from_ne_bytes(*b"x"));
        let full = vec![b'x'; path.len()];
        assert!(with_host(&address, |host| host.to_bytes() == full));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_pton or inet_ntop")]
    fn with_host_text_outlives_other_formatting() {
        let v4 = socket_address(TCP, c"10.1.2.3");
        let v6 = socket_address(TCP6, c"fe80::1");
        with_host(&v4, |outer| {
            // Neither another lend nor the export's shared scratch touches `outer`.
            assert_eq!(with_host(&v6, CStr::to_owned).as_c_str(), c"fe80::1");
            assert!(!swr_v1_clients_address_host(&v6).is_null());
            assert_eq!(outer, c"10.1.2.3");
        });
    }
}
