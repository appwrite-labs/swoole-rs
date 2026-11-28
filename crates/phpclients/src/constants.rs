//! The `ext/sockets` constants `Coroutine\Socket` registers when that extension is
//! not loaded, in upstream's order. Upstream guards most with `#ifdef`; the
//! `cfg`s here follow which of those the target's headers define (Linux: glibc
//! and musl agree on every name below).
use libc::c_int;

/// `(name, value)` in registration order.
type Table = &'static [(&'static std::ffi::CStr, c_int)];

/// Socket families and types, `MSG_*` and `SO_*`.
pub(crate) const SOCKET: Table = &[
    (c"AF_UNIX", libc::AF_UNIX),
    (c"AF_INET", libc::AF_INET),
    (c"AF_INET6", libc::AF_INET6),
    (c"SOCK_STREAM", libc::SOCK_STREAM),
    (c"SOCK_DGRAM", libc::SOCK_DGRAM),
    (c"SOCK_RAW", libc::SOCK_RAW),
    (c"SOCK_SEQPACKET", libc::SOCK_SEQPACKET),
    (c"SOCK_RDM", libc::SOCK_RDM),
    (c"MSG_OOB", libc::MSG_OOB),
    (c"MSG_WAITALL", libc::MSG_WAITALL),
    (c"MSG_CTRUNC", libc::MSG_CTRUNC),
    (c"MSG_TRUNC", libc::MSG_TRUNC),
    (c"MSG_PEEK", libc::MSG_PEEK),
    (c"MSG_DONTROUTE", libc::MSG_DONTROUTE),
    (c"MSG_EOR", libc::MSG_EOR),
    #[cfg(target_vendor = "apple")]
    (c"MSG_EOF", libc::MSG_EOF),
    #[cfg(target_os = "linux")]
    (c"MSG_CONFIRM", libc::MSG_CONFIRM),
    #[cfg(target_os = "linux")]
    (c"MSG_ERRQUEUE", libc::MSG_ERRQUEUE),
    #[cfg(target_os = "linux")]
    (c"MSG_NOSIGNAL", libc::MSG_NOSIGNAL),
    (c"MSG_DONTWAIT", libc::MSG_DONTWAIT),
    #[cfg(target_os = "linux")]
    (c"MSG_MORE", libc::MSG_MORE),
    #[cfg(target_os = "linux")]
    (c"MSG_WAITFORONE", libc::MSG_WAITFORONE),
    #[cfg(target_os = "linux")]
    (c"MSG_CMSG_CLOEXEC", libc::MSG_CMSG_CLOEXEC),
    (c"SO_DEBUG", libc::SO_DEBUG),
    (c"SO_REUSEADDR", libc::SO_REUSEADDR),
    (c"SO_REUSEPORT", libc::SO_REUSEPORT),
    (c"SO_KEEPALIVE", libc::SO_KEEPALIVE),
    (c"SO_DONTROUTE", libc::SO_DONTROUTE),
    (c"SO_LINGER", libc::SO_LINGER),
    (c"SO_BROADCAST", libc::SO_BROADCAST),
    (c"SO_OOBINLINE", libc::SO_OOBINLINE),
    (c"SO_SNDBUF", libc::SO_SNDBUF),
    (c"SO_RCVBUF", libc::SO_RCVBUF),
    (c"SO_SNDLOWAT", libc::SO_SNDLOWAT),
    (c"SO_RCVLOWAT", libc::SO_RCVLOWAT),
    (c"SO_SNDTIMEO", libc::SO_SNDTIMEO),
    (c"SO_RCVTIMEO", libc::SO_RCVTIMEO),
    (c"SO_TYPE", libc::SO_TYPE),
    (c"SO_ERROR", libc::SO_ERROR),
    #[cfg(target_os = "linux")]
    (c"SO_BINDTODEVICE", libc::SO_BINDTODEVICE),
    (c"SOL_SOCKET", libc::SOL_SOCKET),
    (c"SOMAXCONN", libc::SOMAXCONN),
    (c"TCP_NODELAY", libc::TCP_NODELAY),
];

/// The multicast options `thirdparty/php/sockets/multicast.h` maps: the RFC 3678
/// API where the headers have it (not Apple), else the IPv4 options.
#[cfg(not(target_vendor = "apple"))]
pub(crate) const MULTICAST: Table = &[
    (c"MCAST_JOIN_GROUP", libc::MCAST_JOIN_GROUP),
    (c"MCAST_LEAVE_GROUP", libc::MCAST_LEAVE_GROUP),
    (c"MCAST_BLOCK_SOURCE", libc::MCAST_BLOCK_SOURCE),
    (c"MCAST_UNBLOCK_SOURCE", libc::MCAST_UNBLOCK_SOURCE),
    (c"MCAST_JOIN_SOURCE_GROUP", libc::MCAST_JOIN_SOURCE_GROUP),
    (c"MCAST_LEAVE_SOURCE_GROUP", libc::MCAST_LEAVE_SOURCE_GROUP),
];
#[cfg(target_vendor = "apple")]
pub(crate) const MULTICAST: Table = &[
    (c"MCAST_JOIN_GROUP", libc::IP_ADD_MEMBERSHIP),
    (c"MCAST_LEAVE_GROUP", libc::IP_DROP_MEMBERSHIP),
];

/// IP multicast options, then `SOCKET_E*` errno values.
pub(crate) const ERRNO: Table = &[
    (c"IP_MULTICAST_IF", libc::IP_MULTICAST_IF),
    (c"IP_MULTICAST_TTL", libc::IP_MULTICAST_TTL),
    (c"IP_MULTICAST_LOOP", libc::IP_MULTICAST_LOOP),
    (c"IPV6_MULTICAST_IF", libc::IPV6_MULTICAST_IF),
    (c"IPV6_MULTICAST_HOPS", libc::IPV6_MULTICAST_HOPS),
    (c"IPV6_MULTICAST_LOOP", libc::IPV6_MULTICAST_LOOP),
    (c"IPV6_V6ONLY", libc::IPV6_V6ONLY),
    (c"SOCKET_EPERM", libc::EPERM),
    (c"SOCKET_ENOENT", libc::ENOENT),
    (c"SOCKET_EINTR", libc::EINTR),
    (c"SOCKET_EIO", libc::EIO),
    (c"SOCKET_ENXIO", libc::ENXIO),
    (c"SOCKET_E2BIG", libc::E2BIG),
    (c"SOCKET_EBADF", libc::EBADF),
    (c"SOCKET_EAGAIN", libc::EAGAIN),
    (c"SOCKET_ENOMEM", libc::ENOMEM),
    (c"SOCKET_EACCES", libc::EACCES),
    (c"SOCKET_EFAULT", libc::EFAULT),
    (c"SOCKET_ENOTBLK", libc::ENOTBLK),
    (c"SOCKET_EBUSY", libc::EBUSY),
    (c"SOCKET_EEXIST", libc::EEXIST),
    (c"SOCKET_EXDEV", libc::EXDEV),
    (c"SOCKET_ENODEV", libc::ENODEV),
    (c"SOCKET_ENOTDIR", libc::ENOTDIR),
    (c"SOCKET_EISDIR", libc::EISDIR),
    (c"SOCKET_EINVAL", libc::EINVAL),
    (c"SOCKET_ENFILE", libc::ENFILE),
    (c"SOCKET_EMFILE", libc::EMFILE),
    (c"SOCKET_ENOTTY", libc::ENOTTY),
    (c"SOCKET_ENOSPC", libc::ENOSPC),
    (c"SOCKET_ESPIPE", libc::ESPIPE),
    (c"SOCKET_EROFS", libc::EROFS),
    (c"SOCKET_EMLINK", libc::EMLINK),
    (c"SOCKET_EPIPE", libc::EPIPE),
    (c"SOCKET_ENAMETOOLONG", libc::ENAMETOOLONG),
    (c"SOCKET_ENOLCK", libc::ENOLCK),
    (c"SOCKET_ENOSYS", libc::ENOSYS),
    (c"SOCKET_ENOTEMPTY", libc::ENOTEMPTY),
    (c"SOCKET_ELOOP", libc::ELOOP),
    (c"SOCKET_EWOULDBLOCK", libc::EWOULDBLOCK),
    (c"SOCKET_ENOMSG", libc::ENOMSG),
    (c"SOCKET_EIDRM", libc::EIDRM),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ECHRNG", libc::ECHRNG),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EL2NSYNC", libc::EL2NSYNC),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EL3HLT", libc::EL3HLT),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EL3RST", libc::EL3RST),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ELNRNG", libc::ELNRNG),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EUNATCH", libc::EUNATCH),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ENOCSI", libc::ENOCSI),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EL2HLT", libc::EL2HLT),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EBADE", libc::EBADE),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EBADR", libc::EBADR),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EXFULL", libc::EXFULL),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ENOANO", libc::ENOANO),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EBADRQC", libc::EBADRQC),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EBADSLT", libc::EBADSLT),
    (c"SOCKET_ENOSTR", libc::ENOSTR),
    (c"SOCKET_ENODATA", libc::ENODATA),
    (c"SOCKET_ETIME", libc::ETIME),
    (c"SOCKET_ENOSR", libc::ENOSR),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ENONET", libc::ENONET),
    (c"SOCKET_EREMOTE", libc::EREMOTE),
    (c"SOCKET_ENOLINK", libc::ENOLINK),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EADV", libc::EADV),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ESRMNT", libc::ESRMNT),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ECOMM", libc::ECOMM),
    (c"SOCKET_EPROTO", libc::EPROTO),
    (c"SOCKET_EMULTIHOP", libc::EMULTIHOP),
    (c"SOCKET_EBADMSG", libc::EBADMSG),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ENOTUNIQ", libc::ENOTUNIQ),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EBADFD", libc::EBADFD),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EREMCHG", libc::EREMCHG),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ERESTART", libc::ERESTART),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ESTRPIPE", libc::ESTRPIPE),
    (c"SOCKET_EUSERS", libc::EUSERS),
    (c"SOCKET_ENOTSOCK", libc::ENOTSOCK),
    (c"SOCKET_EDESTADDRREQ", libc::EDESTADDRREQ),
    (c"SOCKET_EMSGSIZE", libc::EMSGSIZE),
    (c"SOCKET_EPROTOTYPE", libc::EPROTOTYPE),
    (c"SOCKET_ENOPROTOOPT", libc::ENOPROTOOPT),
    (c"SOCKET_EPROTONOSUPPORT", libc::EPROTONOSUPPORT),
    (c"SOCKET_ESOCKTNOSUPPORT", libc::ESOCKTNOSUPPORT),
    (c"SOCKET_EOPNOTSUPP", libc::EOPNOTSUPP),
    (c"SOCKET_EPFNOSUPPORT", libc::EPFNOSUPPORT),
    (c"SOCKET_EAFNOSUPPORT", libc::EAFNOSUPPORT),
    (c"SOCKET_EADDRINUSE", libc::EADDRINUSE),
    (c"SOCKET_EADDRNOTAVAIL", libc::EADDRNOTAVAIL),
    (c"SOCKET_ENETDOWN", libc::ENETDOWN),
    (c"SOCKET_ENETUNREACH", libc::ENETUNREACH),
    (c"SOCKET_ENETRESET", libc::ENETRESET),
    (c"SOCKET_ECONNABORTED", libc::ECONNABORTED),
    (c"SOCKET_ECONNRESET", libc::ECONNRESET),
    (c"SOCKET_ENOBUFS", libc::ENOBUFS),
    (c"SOCKET_EISCONN", libc::EISCONN),
    (c"SOCKET_ENOTCONN", libc::ENOTCONN),
    (c"SOCKET_ESHUTDOWN", libc::ESHUTDOWN),
    (c"SOCKET_ETOOMANYREFS", libc::ETOOMANYREFS),
    (c"SOCKET_ETIMEDOUT", libc::ETIMEDOUT),
    (c"SOCKET_ECONNREFUSED", libc::ECONNREFUSED),
    (c"SOCKET_EHOSTDOWN", libc::EHOSTDOWN),
    (c"SOCKET_EHOSTUNREACH", libc::EHOSTUNREACH),
    (c"SOCKET_EALREADY", libc::EALREADY),
    (c"SOCKET_EINPROGRESS", libc::EINPROGRESS),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EISNAM", libc::EISNAM),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EREMOTEIO", libc::EREMOTEIO),
    (c"SOCKET_EDQUOT", libc::EDQUOT),
    #[cfg(target_os = "linux")]
    (c"SOCKET_ENOMEDIUM", libc::ENOMEDIUM),
    #[cfg(target_os = "linux")]
    (c"SOCKET_EMEDIUMTYPE", libc::EMEDIUMTYPE),
    (c"IPPROTO_IP", libc::IPPROTO_IP),
    (c"IPPROTO_IPV6", libc::IPPROTO_IPV6),
    (c"SOL_TCP", libc::IPPROTO_TCP),
    (c"SOL_UDP", libc::IPPROTO_UDP),
    (c"IPV6_UNICAST_HOPS", libc::IPV6_UNICAST_HOPS),
    (c"AI_PASSIVE", libc::AI_PASSIVE),
    (c"AI_CANONNAME", libc::AI_CANONNAME),
    (c"AI_NUMERICHOST", libc::AI_NUMERICHOST),
];

/// `AI_V4MAPPED`, `AI_ALL` and `AI_IDN`/`AI_CANONIDN` follow PHP's configure
/// results (`HAVE_AI_*`), then `AI_ADDRCONFIG` and `AI_NUMERICSERV`.
pub(crate) const AI_V4MAPPED: (&std::ffi::CStr, c_int) = (c"AI_V4MAPPED", libc::AI_V4MAPPED);
pub(crate) const AI_ALL: (&std::ffi::CStr, c_int) = (c"AI_ALL", libc::AI_ALL);
pub(crate) const AI_ADDRCONFIG: (&std::ffi::CStr, c_int) = (c"AI_ADDRCONFIG", libc::AI_ADDRCONFIG);
/// glibc's `<netdb.h>` values; PHP only finds `AI_IDN` where glibc defines them.
pub(crate) const AI_IDN: Table = &[(c"AI_IDN", 0x0040), (c"AI_CANONIDN", 0x0080)];
pub(crate) const AI_NUMERICSERV: (&std::ffi::CStr, c_int) =
    (c"AI_NUMERICSERV", libc::AI_NUMERICSERV);

/// Registered whether or not `ext/sockets` is loaded.
pub(crate) const ALWAYS: Table = &[
    (c"SOCKET_ECANCELED", libc::ECANCELED),
    #[cfg(target_os = "linux")]
    (c"TCP_INFO", libc::TCP_INFO),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = [SOCKET, MULTICAST, ERRNO, AI_IDN, ALWAYS]
            .iter()
            .flat_map(|table| table.iter().map(|(name, _)| *name))
            .chain([AI_V4MAPPED.0, AI_ALL.0, AI_ADDRCONFIG.0, AI_NUMERICSERV.0])
            .collect();
        let count = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), count, "each constant registers once");
    }
}
