//! `network::Socket`'s mapping between `swSocketType` and (domain, type), and the
//! type a host string names.
use std::os::raw::c_int;
use swoole_rs_corebase::constants::SocketType::{
    self, SW_SOCK_RAW as RAW, SW_SOCK_RAW6 as RAW6, SW_SOCK_TCP as TCP, SW_SOCK_TCP6 as TCP6,
    SW_SOCK_UDP as UDP, SW_SOCK_UDP6 as UDP6, SW_SOCK_UNIX_DGRAM as UNIX_DGRAM,
    SW_SOCK_UNIX_STREAM as UNIX_STREAM,
};

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
    .raw()
}

/// `Socket::get_domain_and_type`: false for a type with no socket(2) arguments.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_socket_domain_and_type(
    socket_type: c_int,
    domain: &mut c_int,
    kind: &mut c_int,
) -> bool {
    let Some(socket_type) = SocketType::from_raw(socket_type) else {
        return false;
    };
    let (d, k) = match socket_type {
        TCP6 => (libc::AF_INET6, libc::SOCK_STREAM),
        UNIX_STREAM => (libc::AF_UNIX, libc::SOCK_STREAM),
        UDP => (libc::AF_INET, libc::SOCK_DGRAM),
        UDP6 => (libc::AF_INET6, libc::SOCK_DGRAM),
        UNIX_DGRAM => (libc::AF_UNIX, libc::SOCK_DGRAM),
        TCP => (libc::AF_INET, libc::SOCK_STREAM),
        RAW => (libc::AF_INET, libc::SOCK_RAW),
        RAW6 => (libc::AF_INET6, libc::SOCK_RAW),
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
            socket_type: UNIX_STREAM.raw(),
            erase,
        };
    }
    HostType {
        socket_type: if host.contains(&b':') { TCP6 } else { TCP }.raw(),
        erase: 0,
    }
}

/// `SW_SOCK_NONBLOCK` and `SW_SOCK_CLOEXEC`.
pub(crate) const NONBLOCK: c_int =
    swoole_rs_corebase::constants::SocketFlag::SW_SOCK_NONBLOCK as c_int;
pub(crate) const CLOEXEC: c_int =
    swoole_rs_corebase::constants::SocketFlag::SW_SOCK_CLOEXEC as c_int;

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
    let (nonblock, cloexec) = (flags & NONBLOCK != 0, flags & CLOEXEC != 0);
    #[cfg(target_os = "linux")]
    {
        let extra = if nonblock { libc::SOCK_NONBLOCK } else { 0 }
            | if cloexec { libc::SOCK_CLOEXEC } else { 0 };
        swoole_rs_corebase::sys::socket(domain, kind | extra, protocol)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let fd = swoole_rs_corebase::sys::socket(domain, kind, protocol)?;
        if (nonblock || cloexec)
            && !crate::swr_v1_socket_fd_option(
                fd,
                nonblock.then_some(true),
                cloexec.then_some(true),
            )
            .ok
        {
            _ = swoole_rs_corebase::sys::close(fd);
        }
        Ok(fd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_and_type_round_trip() {
        for socket_type in TCP.raw()..=RAW6.raw() {
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
        assert_eq!(swr_v1_socket_type(libc::AF_UNIX, libc::SOCK_RAW), RAW.raw());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no network sockets")]
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
            (
                SocketType::from_raw(plan.socket_type),
                &host[plan.erase.min(host.len())..],
            )
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
