//! `network::Socket`'s mapping between `swSocketType` and (domain, type), and the
//! type a host string names.
use crate::constants::SocketFlag;
use crate::constants::SocketType::{
    self, SW_SOCK_RAW as RAW, SW_SOCK_RAW6 as RAW6, SW_SOCK_TCP as TCP, SW_SOCK_TCP6 as TCP6,
    SW_SOCK_UDP as UDP, SW_SOCK_UDP6 as UDP6, SW_SOCK_UNIX_DGRAM as UNIX_DGRAM,
    SW_SOCK_UNIX_STREAM as UNIX_STREAM,
};
use std::os::raw::c_int;

/// `Socket::convert_to_type(domain, type)`: anything unknown is `SW_SOCK_RAW`.
#[must_use]
pub fn socket_type(domain: c_int, kind: c_int) -> SocketType {
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

/// `Socket::get_domain_and_type`: socket(2)'s domain and type, none for an unknown type.
#[must_use]
pub fn socket_domain_and_type(socket_type: c_int) -> Option<(c_int, c_int)> {
    Some(match SocketType::from_raw(socket_type)? {
        TCP6 => (libc::AF_INET6, libc::SOCK_STREAM),
        UNIX_STREAM => (libc::AF_UNIX, libc::SOCK_STREAM),
        UDP => (libc::AF_INET, libc::SOCK_DGRAM),
        UDP6 => (libc::AF_INET6, libc::SOCK_DGRAM),
        UNIX_DGRAM => (libc::AF_UNIX, libc::SOCK_DGRAM),
        TCP => (libc::AF_INET, libc::SOCK_STREAM),
        RAW => (libc::AF_INET, libc::SOCK_RAW),
        RAW6 => (libc::AF_INET6, libc::SOCK_RAW),
    })
}

/// The type a host string names, and how many leading bytes `convert_to_type`
/// erases from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostType {
    pub socket_type: SocketType,
    pub erase: usize,
}

/// `Socket::convert_to_type(host)` over `host`'s bytes.
#[must_use]
pub fn host_type(host: &[u8]) -> HostType {
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

/// `SW_SOCK_NONBLOCK` and `SW_SOCK_CLOEXEC`.
pub(crate) const NONBLOCK: c_int = crate::constants::SocketFlag::SW_SOCK_NONBLOCK.bits();
pub(crate) const CLOEXEC: c_int = crate::constants::SocketFlag::SW_SOCK_CLOEXEC.bits();

/// `swoole::socket`: socket(2) with `SW_SOCK_NONBLOCK`/`SW_SOCK_CLOEXEC` applied,
/// atomically where the platform has `SOCK_NONBLOCK`/`SOCK_CLOEXEC`, else by fcntl.
/// As upstream, a failed fcntl closes the socket but still returns its number.
///
/// # Errors
/// socket(2)'s error.
pub fn swr_v1_socket_open(
    domain: c_int,
    kind: c_int,
    protocol: c_int,
    flags: c_int,
) -> std::io::Result<c_int> {
    let flags = SocketFlag::from_bits(flags);
    let (nonblock, cloexec) = (
        flags.contains(SocketFlag::SW_SOCK_NONBLOCK),
        flags.contains(SocketFlag::SW_SOCK_CLOEXEC),
    );
    #[cfg(target_os = "linux")]
    {
        let extra = if nonblock { libc::SOCK_NONBLOCK } else { 0 }
            | if cloexec { libc::SOCK_CLOEXEC } else { 0 };
        crate::sys::socket(domain, kind | extra, protocol)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let fd = crate::sys::socket(domain, kind, protocol)?;
        if (nonblock || cloexec)
            && !crate::reactor::swr_v1_socket_fd_option(
                fd,
                nonblock.then_some(true),
                cloexec.then_some(true),
            )
            .ok
        {
            _ = crate::sys::close(fd);
        }
        Ok(fd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_and_type_round_trip() {
        for raw in TCP.raw()..=RAW6.raw() {
            let (domain, kind) = socket_domain_and_type(raw).expect("a known type");
            assert_eq!(socket_type(domain, kind).raw(), raw);
        }
        assert_eq!(socket_domain_and_type(0), None);
        assert_eq!(socket_type(libc::AF_UNIX, libc::SOCK_RAW), RAW);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri aborts on an unsupported socket domain")]
    fn open_applies_the_flags() {
        let fd = swr_v1_socket_open(libc::AF_INET, libc::SOCK_STREAM, 0, NONBLOCK | CLOEXEC)
            .expect("a socket");
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
            swr_v1_socket_open(-1, libc::SOCK_STREAM, 0, 0).is_err(),
            "bad domain"
        );
    }

    #[test]
    fn hosts_name_their_type() {
        fn erased(host: &[u8]) -> (Option<SocketType>, &[u8]) {
            let plan = host_type(host);
            (Some(plan.socket_type), &host[plan.erase.min(host.len())..])
        }
        assert_eq!(erased(b"unix:/tmp/s"), (Some(UNIX_STREAM), &b"/tmp/s"[..]));
        assert_eq!(
            erased(b"unix:///tmp/s"),
            (Some(UNIX_STREAM), &b"/tmp/s"[..])
        );
        assert_eq!(erased(b"unix://"), (Some(UNIX_STREAM), &b""[..]));
        assert_eq!(erased(b"::1"), (Some(TCP6), &b"::1"[..]));
        assert_eq!(erased(b"127.0.0.1"), (Some(TCP), &b"127.0.0.1"[..]));
        assert_eq!(erased(b"unix:x"), (Some(TCP6), &b"unix:x"[..]));
    }
}
