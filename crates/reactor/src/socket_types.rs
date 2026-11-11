//! `network::Socket`'s mapping between `swSocketType` and (domain, type), and the
//! type a host string names.
use std::os::raw::c_int;

/// `swSocketType`.
const TCP: c_int = 1;
const UDP: c_int = 2;
const TCP6: c_int = 3;
const UDP6: c_int = 4;
const UNIX_STREAM: c_int = 5;
const UNIX_DGRAM: c_int = 6;
const RAW: c_int = 7;
const RAW6: c_int = 8;

/// `Socket::is_stream`, `is_dgram`, `is_inet4`, `is_inet6`, `is_tcp` and `is_local`:
/// the bits [`swr_v1_socket_type_classes`] holds for each `swSocketType`.
pub const CLASS_STREAM: u8 = 1;
pub const CLASS_DGRAM: u8 = 1 << 1;
pub const CLASS_INET4: u8 = 1 << 2;
pub const CLASS_INET6: u8 = 1 << 3;
pub const CLASS_TCP: u8 = 1 << 4;
pub const CLASS_LOCAL: u8 = 1 << 5;

/// Each `swSocketType`'s classes, indexed by the type; native code reads it inline.
#[unsafe(no_mangle)]
#[expect(
    clippy::indexing_slicing,
    reason = "const-evaluated: an out-of-range index fails the build, and const has no get()"
)]
pub static swr_v1_socket_type_classes: [u8; 16] = {
    let mut classes = [0; 16];
    classes[TCP as usize] = CLASS_STREAM | CLASS_INET4 | CLASS_TCP;
    classes[TCP6 as usize] = CLASS_STREAM | CLASS_INET6 | CLASS_TCP;
    classes[UNIX_STREAM as usize] = CLASS_STREAM | CLASS_LOCAL;
    classes[UDP as usize] = CLASS_DGRAM | CLASS_INET4;
    classes[UDP6 as usize] = CLASS_DGRAM | CLASS_INET6;
    classes[UNIX_DGRAM as usize] = CLASS_DGRAM | CLASS_LOCAL;
    classes[RAW as usize] = CLASS_INET4;
    classes[RAW6 as usize] = CLASS_INET6;
    classes
};

/// Whether `socket_type` has any of `classes`; unknown types have none.
#[must_use]
pub fn type_is(socket_type: c_int, classes: u8) -> bool {
    usize::try_from(socket_type)
        .ok()
        .and_then(|index| swr_v1_socket_type_classes.get(index))
        .is_some_and(|bits| bits & classes != 0)
}

/// `Socket::convert_to_type(domain, type)`: anything unknown is `SW_SOCK_RAW`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_type(domain: c_int, kind: c_int) -> c_int {
    match (domain, kind) {
        (libc::AF_INET, libc::SOCK_STREAM) => TCP,
        (libc::AF_INET6, libc::SOCK_STREAM) => TCP6,
        (libc::AF_UNIX, libc::SOCK_STREAM) => UNIX_STREAM,
        (libc::AF_INET, libc::SOCK_DGRAM) => UDP,
        (libc::AF_INET6, libc::SOCK_DGRAM) => UDP6,
        (libc::AF_UNIX, libc::SOCK_DGRAM) => UNIX_DGRAM,
        (libc::AF_INET6, libc::SOCK_RAW) => RAW6,
        _ => RAW,
    }
}

/// `Socket::get_domain_and_type`: false for a type with no socket(2) arguments.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_domain_and_type(
    socket_type: c_int,
    domain: &mut c_int,
    kind: &mut c_int,
) -> bool {
    let (d, k) = match socket_type {
        TCP6 => (libc::AF_INET6, libc::SOCK_STREAM),
        UNIX_STREAM => (libc::AF_UNIX, libc::SOCK_STREAM),
        UDP => (libc::AF_INET, libc::SOCK_DGRAM),
        UDP6 => (libc::AF_INET6, libc::SOCK_DGRAM),
        UNIX_DGRAM => (libc::AF_UNIX, libc::SOCK_DGRAM),
        TCP => (libc::AF_INET, libc::SOCK_STREAM),
        RAW => (libc::AF_INET, libc::SOCK_RAW),
        RAW6 => (libc::AF_INET6, libc::SOCK_RAW),
        _ => return false,
    };
    *domain = d;
    *kind = k;
    true
}

/// The type a host string names, and how many leading bytes `convert_to_type`
/// erases from it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostType {
    pub socket_type: c_int,
    pub erase: usize,
}

fn host_type(host: &[u8]) -> HostType {
    if host.starts_with(b"unix:/") {
        // "unix:" goes, then every leading '/' but one; all slashes erases everything.
        let path = host.get(5..).unwrap_or_default();
        let erase = path
            .iter()
            .position(|&b| b != b'/')
            .map_or(host.len(), |first| 5 + first - 1);
        return HostType {
            socket_type: UNIX_STREAM,
            erase,
        };
    }
    HostType {
        socket_type: if host.contains(&b':') { TCP6 } else { TCP },
        erase: 0,
    }
}

/// `Socket::convert_to_type(host)`: `unix:/path` is a Unix stream socket, a ':' is
/// IPv6, anything else IPv4 TCP.
///
/// # Safety
/// `host` denotes `length` readable bytes, or is null for length 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_host_type(host: *const u8, length: usize) -> HostType {
    if length == 0 || host.is_null() || length > isize::MAX as usize {
        return host_type(&[]);
    }
    // SAFETY: `host` is non-null and `length <= isize::MAX` (checked above); the caller lends `length` bytes.
    host_type(unsafe { std::slice::from_raw_parts(host, length) })
}

/// `SW_SOCK_NONBLOCK` and `SW_SOCK_CLOEXEC`.
pub(crate) const NONBLOCK: c_int = 1 << 2;
pub(crate) const CLOEXEC: c_int = 1 << 3;

/// `swoole::socket`: socket(2) with `SW_SOCK_NONBLOCK`/`SW_SOCK_CLOEXEC` applied,
/// atomically where the platform has `SOCK_NONBLOCK`/`SOCK_CLOEXEC`, else by fcntl.
/// As upstream, a failed fcntl closes the socket but still returns its number.
#[must_use]
pub fn swr_v1_socket_open(domain: c_int, kind: c_int, protocol: c_int, flags: c_int) -> c_int {
    let (nonblock, cloexec) = (flags & NONBLOCK != 0, flags & CLOEXEC != 0);
    #[cfg(target_os = "linux")]
    {
        let extra = if nonblock { libc::SOCK_NONBLOCK } else { 0 }
            | if cloexec { libc::SOCK_CLOEXEC } else { 0 };
        // SAFETY: socket(2) has no memory preconditions.
        unsafe { libc::socket(domain, kind | extra, protocol) }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: socket(2) has no memory preconditions.
        let fd = unsafe { libc::socket(domain, kind, protocol) };
        if fd >= 0
            && (nonblock || cloexec)
            && !crate::swr_v1_socket_fd_option(
                fd,
                if nonblock { 1 } else { -1 },
                if cloexec { 1 } else { -1 },
            )
            .ok
        {
            swoole_rs_corebase::sys::close(fd);
        }
        fd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_match_upstream_predicates() {
        let stream = [TCP, TCP6, UNIX_STREAM];
        let dgram = [UDP, UDP6, UNIX_DGRAM];
        let inet4 = [TCP, UDP, RAW];
        let inet6 = [TCP6, UDP6, RAW6];
        for socket_type in -1..20 {
            assert_eq!(
                type_is(socket_type, CLASS_STREAM),
                stream.contains(&socket_type)
            );
            assert_eq!(
                type_is(socket_type, CLASS_DGRAM),
                dgram.contains(&socket_type)
            );
            assert_eq!(
                type_is(socket_type, CLASS_INET4),
                inet4.contains(&socket_type)
            );
            assert_eq!(
                type_is(socket_type, CLASS_INET6),
                inet6.contains(&socket_type)
            );
            assert_eq!(
                type_is(socket_type, CLASS_TCP),
                [TCP, TCP6].contains(&socket_type)
            );
            assert_eq!(
                type_is(socket_type, CLASS_LOCAL),
                [UNIX_STREAM, UNIX_DGRAM].contains(&socket_type)
            );
        }
    }

    #[test]
    fn domain_and_type_round_trip() {
        for socket_type in TCP..=RAW6 {
            let (mut domain, mut kind) = (0, 0);
            assert!(swr_v1_socket_domain_and_type(
                socket_type,
                &mut domain,
                &mut kind
            ));
            assert_eq!(swr_v1_socket_type(domain, kind), socket_type);
        }
        let (mut domain, mut kind) = (0, 0);
        assert!(!swr_v1_socket_domain_and_type(0, &mut domain, &mut kind));
        assert_eq!(swr_v1_socket_type(libc::AF_UNIX, libc::SOCK_RAW), RAW);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no network sockets")]
    fn open_applies_the_flags() {
        let fd = swr_v1_socket_open(libc::AF_INET, libc::SOCK_STREAM, 0, NONBLOCK | CLOEXEC);
        assert!(fd >= 0, "a socket");
        // SAFETY: fcntl(F_GETFL/F_GETFD) on a socket this test opened.
        unsafe {
            assert_ne!(
                libc::fcntl(fd, libc::F_GETFL) & libc::O_NONBLOCK,
                0,
                "nonblocking"
            );
            assert_ne!(
                libc::fcntl(fd, libc::F_GETFD) & libc::FD_CLOEXEC,
                0,
                "close-on-exec"
            );
            libc::close(fd);
        }
        assert!(
            swr_v1_socket_open(-1, libc::SOCK_STREAM, 0, 0) < 0,
            "bad domain"
        );
    }

    #[test]
    fn hosts_name_their_type() {
        fn erased(host: &[u8]) -> (c_int, &[u8]) {
            let plan = host_type(host);
            (plan.socket_type, &host[plan.erase.min(host.len())..])
        }
        assert_eq!(erased(b"unix:/tmp/s"), (UNIX_STREAM, &b"/tmp/s"[..]));
        assert_eq!(erased(b"unix:///tmp/s"), (UNIX_STREAM, &b"/tmp/s"[..]));
        assert_eq!(erased(b"unix://"), (UNIX_STREAM, &b""[..]));
        assert_eq!(erased(b"::1"), (TCP6, &b"::1"[..]));
        assert_eq!(erased(b"127.0.0.1"), (TCP, &b"127.0.0.1"[..]));
        assert_eq!(erased(b"unix:x"), (TCP6, &b"unix:x"[..]));
    }
}
