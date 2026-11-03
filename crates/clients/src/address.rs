//! `network::Address::assign(type, host, port, resolve_name)`: fill a native
//! `Address`'s sockaddr union and length for an IPv4, IPv6 or Unix socket type, parsing
//! the host as an IP literal or, if allowed, resolving it through native
//! `gethostbyname`. Native code keeps `type`, set before the call, and the last error.
use crate::ip::swr_v1_clients_ip;
use crate::swr_v1_clients_port;
use std::ffi::{c_char, c_int, c_void};

/// `SocketType` values from `swoole.h`.
const TCP: c_int = 1;
const UDP: c_int = 2;
const TCP6: c_int = 3;
const UDP6: c_int = 4;
const UNIX_STREAM: c_int = 5;
const UNIX_DGRAM: c_int = 6;
const RAW: c_int = 7;
const RAW6: c_int = 8;

/// `SW_ERROR_*` codes `assign` sets as the last error.
const NAME_TOO_LONG: c_int = 504;
const DNSLOOKUP_RESOLVE_FAILED: c_int = 711;
const BAD_HOST_ADDR: c_int = 722;
const BAD_PORT: c_int = 723;
const BAD_SOCKET_TYPE: c_int = 724;

unsafe extern "C" {
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
    // Other families, and Apple's IPv6 scope names (resolved as it parses, changing
    // errno), go to the platform.
    let apple_scope = cfg!(target_os = "macos") && family == libc::AF_INET6 && text.contains(&b'%');
    if (family != libc::AF_INET && family != libc::AF_INET6) || apple_scope {
        // SAFETY: as above.
        return unsafe { inet_pton(family, host, destination.cast()) } == 1;
    }
    let version = if family == libc::AF_INET { 4 } else { 6 };
    let allow_zeroes = u8::from(cfg!(target_os = "macos"));
    // SAFETY: `text` is the host's bytes.
    let ip = unsafe { swr_v1_clients_ip(version, text.as_ptr(), text.len(), allow_zeroes) };
    if ip.length == 0 {
        return false;
    }
    let length = usize::try_from(ip.length).unwrap_or(0);
    // SAFETY: `destination` holds the family's `length`-byte address.
    unsafe { std::ptr::copy_nonoverlapping(ip.bytes.as_ptr(), destination, length) };
    true
}

/// `Address::verify_ip`: whether `host` is an IP literal of `family`, parsed into
/// `destination` (native's scratch `tmp_address`).
///
/// # Safety
/// `host` is NUL-terminated; `destination` holds any family's address.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_verify_ip(
    family: c_int,
    host: *const c_char,
    destination: *mut u8,
) -> bool {
    // SAFETY: as the caller promises.
    unsafe { parse_ip(family, host, destination) }
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
    if swr_v1_clients_port(port, 0) == 0 {
        return BAD_PORT;
    }
    // In range: `swr_v1_clients_port` accepted it.
    let port = u16::try_from(port).unwrap_or(0).to_be();
    let size = |of: usize| libc::socklen_t::try_from(of).unwrap_or(0);
    match kind {
        TCP | UDP | RAW => {
            let v4 = address.cast::<libc::sockaddr_in>();
            // SAFETY: `address` is the union, which holds a sockaddr_in.
            let v4 = unsafe { &mut *v4 };
            v4.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap_or(0);
            v4.sin_port = port;
            // SAFETY: the caller passes the union's length.
            unsafe { *length = size(size_of::<libc::sockaddr_in>()) };
            let destination = (&raw mut v4.sin_addr).cast();
            // SAFETY: `sin_addr` holds an IPv4 address.
            unsafe { host_address(libc::AF_INET, host, destination, resolve) }
        }
        TCP6 | UDP6 | RAW6 => {
            let v6 = address.cast::<libc::sockaddr_in6>();
            // SAFETY: `address` is the union, which holds a sockaddr_in6.
            let v6 = unsafe { &mut *v6 };
            v6.sin6_family = libc::sa_family_t::try_from(libc::AF_INET6).unwrap_or(0);
            v6.sin6_port = port;
            // SAFETY: as above.
            unsafe { *length = size(size_of::<libc::sockaddr_in6>()) };
            let destination = (&raw mut v6.sin6_addr).cast();
            // SAFETY: `sin6_addr` holds an IPv6 address.
            unsafe { host_address(libc::AF_INET6, host, destination, resolve) }
        }
        UNIX_STREAM | UNIX_DGRAM => {
            let un = address.cast::<libc::sockaddr_un>();
            // SAFETY: `address` is the union, which holds a sockaddr_un.
            let un = unsafe { &mut *un };
            let path = &mut un.sun_path;
            if host_length >= path.len() - 1 {
                return NAME_TOO_LONG;
            }
            un.sun_family = libc::sa_family_t::try_from(libc::AF_UNIX).unwrap_or(0);
            // SAFETY: the caller passes a NUL-terminated host.
            let text = unsafe { std::ffi::CStr::from_ptr(host) }.to_bytes();
            // `swoole_strlcpy` and the forced final NUL.
            let copied = text.len().min(path.len() - 1);
            for (slot, &byte) in path.iter_mut().zip(&text[..copied]) {
                *slot = c_char::from_ne_bytes([byte]);
            }
            path[copied] = 0;
            *path.last_mut().unwrap_or(&mut 0) = 0;
            // SAFETY: as above.
            unsafe { *length = size(path.len()) };
            0
        }
        _ => BAD_SOCKET_TYPE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut scratch = [0u8; 46];
        unsafe {
            assert!(swr_v1_clients_verify_ip(
                libc::AF_INET,
                c"10.1.2.3".as_ptr(),
                scratch.as_mut_ptr()
            ));
            assert!(!swr_v1_clients_verify_ip(
                libc::AF_INET,
                c"::1".as_ptr(),
                scratch.as_mut_ptr()
            ));
            assert!(swr_v1_clients_verify_ip(
                libc::AF_INET6,
                c"fe80::1".as_ptr(),
                scratch.as_mut_ptr()
            ));
            assert!(!swr_v1_clients_verify_ip(
                libc::AF_UNIX,
                c"10.1.2.3".as_ptr(),
                scratch.as_mut_ptr()
            ));
        }
    }
}
