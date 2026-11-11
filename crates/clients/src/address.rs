//! `network::Address`'s operations for native code. `assign` is crates/corebase's,
//! with a host that is not an IP literal going to the platform or, when allowed,
//! native `gethostbyname`. Native code keeps `type`, set before the call, and the
//! last error.
use crate::ip::swr_v1_clients_ip;
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_int, c_void};
use swoole_rs_corebase::net_address::{
    BAD_HOST_ADDR, DNSLOOKUP_RESOLVE_FAILED, RAW, RAW6, TCP, TCP6, UDP, UDP6, UNIX_DGRAM,
    UNIX_STREAM, platform_parses,
};
use swoole_rs_corebase::net_types::{AddressStorage, SocketAddress};

unsafe extern "C" {
    fn inet_ntop(
        family: c_int,
        address: *const c_void,
        text: *mut c_char,
        size: libc::socklen_t,
    ) -> *const c_char;
    fn inet_pton(family: c_int, source: *const c_char, destination: *mut c_void) -> c_int;
}

/// `network::gethostbyname(family, name, address)`: writes the address, < 0 on failure.
pub type Resolve = unsafe extern "C" fn(c_int, *const c_char, *mut c_char) -> c_int;

/// `clients_parse_ip`: the host as an IP literal of `family` into `destination`.
///
/// # Safety
/// `host` is NUL-terminated and `destination` holds the family's address.
unsafe fn parse_ip(family: c_int, host: *const c_char, destination: *mut u8) -> bool {
    // SAFETY: the caller passes a NUL-terminated host.
    let text = unsafe { std::ffi::CStr::from_ptr(host) }.to_bytes();
    // Other families, and the hosts crates/corebase leaves to it, go to the platform.
    if (family != libc::AF_INET && family != libc::AF_INET6) || platform_parses(family, text) {
        // SAFETY: as above.
        return unsafe { inet_pton(family, host, destination.cast()) } == 1;
    }
    let version = if family == libc::AF_INET { 4 } else { 6 };
    let allow_zeroes = u8::from(cfg!(target_os = "macos"));
    let ip = swr_v1_clients_ip(version, text, allow_zeroes);
    if ip.length == 0 {
        return false;
    }
    let length = usize::try_from(ip.length).unwrap_or(0);
    // SAFETY: `destination` holds the family's `length`-byte address.
    unsafe { std::ptr::copy_nonoverlapping(ip.bytes.as_ptr(), destination, length) };
    true
}

/// An IP family's literal, else (when `resolve` is given) its resolved address.
///
/// # Safety
/// As for [`parse_ip`]; `resolve` is native `gethostbyname`.
unsafe fn host_address(
    family: c_int,
    host: *const c_char,
    destination: *mut u8,
    resolve: Option<Resolve>,
) -> c_int {
    // SAFETY: as the caller promises.
    if unsafe { parse_ip(family, host, destination) } {
        return 0;
    }
    let Some(resolve) = resolve else {
        return BAD_HOST_ADDR;
    };
    // SAFETY: as above.
    if unsafe { resolve(family, host, destination.cast()) } < 0 {
        return DNSLOOKUP_RESOLVE_FAILED;
    }
    0
}

/// Fills `address` (the native sockaddr union) and `*length` for `kind`, `host` and
/// `port`; 0, or the `SW_ERROR_*` code for native to set as the last error.
///
/// # Safety
/// `address` points to the union, `length` to its `socklen_t`, `host` to a
/// NUL-terminated string of `host_length` bytes before any embedded NUL; `resolve`
/// is native `gethostbyname` or null (`resolve_name` false).
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
    // SAFETY: and `host_length` bytes of host.
    let bytes = unsafe { std::slice::from_raw_parts(host.cast::<u8>(), host_length) };
    swoole_rs_corebase::net_address::assign(storage, size, kind, bytes, port, |family| {
        let mut address = [0; 16];
        // SAFETY: the NUL-terminated host and room for either family's address.
        match unsafe { host_address(family, host, address.as_mut_ptr(), resolve) } {
            0 => Ok(address),
            error => Err(error),
        }
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

/// `Address::addr_str`, including the original thread-local returned pointer.
/// # Safety
/// `address` points to the address bytes for `family`, as required by `inet_ntop`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_address_string(
    family: c_int,
    address: *const c_void,
) -> *const c_char {
    ADDRESS_SCRATCH.with(|scratch| {
        // SAFETY: this thread alone accesses its fixed scratch; inet_ntop writes
        // at most its supplied capacity and does not retain the address.
        unsafe { inet_ntop(family, address, scratch.get().cast(), 46) }
    })
}

/// `Address::verify_ip`, writing the same scratch as `addr_str` does.
/// # Safety
/// `host` is a live NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_address_verify(family: c_int, host: *const c_char) -> bool {
    ADDRESS_SCRATCH.with(|scratch| {
        // SAFETY: the scratch holds the largest IP address; this thread alone
        // accesses it and parse_ip retains neither input nor destination.
        unsafe { parse_ip(family, host, scratch.get().cast()) }
    })
}

/// `Address::get_addr`: IP text in shared scratch, Unix path in the record, or
/// the original static fallback for an unknown socket type. The Unix path
/// remains valid only while the record does; IP text lives until reused.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_host(address: &SocketAddress) -> *const c_char {
    if inet4(address.kind) {
        // SAFETY: inet4 selects the initialized IPv4 union member.
        let ip = unsafe { &raw const address.addr.inet_v4.sin_addr };
        // SAFETY: the field holds the selected family's address.
        unsafe { swr_v1_clients_address_string(libc::AF_INET, ip.cast()) }
    } else if inet6(address.kind) {
        // SAFETY: inet6 selects the initialized IPv6 union member.
        let ip = unsafe { &raw const address.addr.inet_v6.sin6_addr };
        // SAFETY: the field holds the selected family's address.
        unsafe { swr_v1_clients_address_string(libc::AF_INET6, ip.cast()) }
    } else if matches!(address.kind, UNIX_STREAM | UNIX_DGRAM) {
        // SAFETY: a local socket selects the initialized Unix union member.
        unsafe { (&raw const address.addr.un.sun_path).cast() }
    } else {
        c"unknown".as_ptr()
    }
}

/// `Address::get_port`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_port(address: &SocketAddress) -> c_int {
    let port = if inet4(address.kind) {
        // SAFETY: inet4 selects the initialized IPv4 union member.
        unsafe { address.addr.inet_v4.sin_port }
    } else if inet6(address.kind) {
        // SAFETY: inet6 selects the initialized IPv6 union member.
        unsafe { address.addr.inet_v6.sin6_port }
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
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_type(kind: c_int) -> *const c_char {
    if inet4(kind) {
        c"IPv4".as_ptr()
    } else if inet6(kind) {
        c"IPv6".as_ptr()
    } else if matches!(kind, UNIX_STREAM | UNIX_DGRAM) {
        c"UnixSocket".as_ptr()
    } else {
        c"Unknown".as_ptr()
    }
}

/// `Address::is_loopback_addr`, preserving exact IPv4 localhost semantics.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_clients_address_loopback(address: &SocketAddress) -> bool {
    let loopback = crate::ip::swr_v1_clients_loopback;
    if inet4(address.kind) {
        // SAFETY: inet4 selects the initialized IPv4 union member.
        let ip = unsafe { address.addr.inet_v4.sin_addr.s_addr };
        loopback(4, &ip.to_ne_bytes()) != 0
    } else if inet6(address.kind) {
        // SAFETY: inet6 selects the initialized IPv6 union member.
        let ip = unsafe { address.addr.inet_v6.sin6_addr.s6_addr };
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
}
