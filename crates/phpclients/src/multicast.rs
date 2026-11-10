//! `Coroutine\Socket::setOption` and `getOption` at `IPPROTO_IP` and
//! `IPPROTO_IPV6`: multicast membership, interface, TTL, hops and loop, and
//! `IPV6_PKTINFO`, as the ext/sockets helpers upstream copies
//! (`thirdparty/php/sockets`) handle them. Each returns upstream's `SUCCESS`
//! (0), `FAILURE` (-1) or 1 for an option it leaves to the caller, and
//! `Err(Stop)` once a warning's handler bailed out.
use crate::{
    native::Co,
    socket_options::{Stop, checked, errno, fd, long, string, text},
};
use libc::{c_char, c_int, c_uint, in_addr, in6_addr, sockaddr_in, sockaddr_in6, socklen_t};
use std::{
    ffi::{CStr, c_void},
    mem::{MaybeUninit, size_of},
    ptr,
};
use swoole_rs_zend::{
    Array, Value, bailout, php_error,
    sys::{E_WARNING, IS_LONG, swr_zend_convert_to_array, swr_zend_is_numeric_string, zval},
};

const SUCCESS: c_int = 0;
const FAILURE: c_int = -1;
const UNHANDLED: c_int = 1;
/// `_php_mcast_*`: failed, and the warning is already out.
const REPORTED: c_int = -2;
/// `INET6_ADDRSTRLEN`.
#[cfg(not(target_vendor = "apple"))]
const INET6_ADDRSTRLEN: usize = 46;
/// `MAXFQDNLEN`.
const MAXFQDNLEN: usize = 255;

/// The membership options `php_do_mcast_opt` takes, and whether each names a
/// source.
#[cfg(not(target_vendor = "apple"))]
const MEMBERSHIP: &[(c_int, bool)] = &[
    (libc::MCAST_JOIN_GROUP, false),
    (libc::MCAST_LEAVE_GROUP, false),
    (libc::MCAST_BLOCK_SOURCE, true),
    (libc::MCAST_UNBLOCK_SOURCE, true),
    (libc::MCAST_JOIN_SOURCE_GROUP, true),
    (libc::MCAST_LEAVE_SOURCE_GROUP, true),
];
#[cfg(target_vendor = "apple")]
const MEMBERSHIP: &[(c_int, bool)] = &[
    (libc::IP_ADD_MEMBERSHIP, false),
    (libc::IP_DROP_MEMBERSHIP, false),
];

unsafe extern "C" {
    /// PHP's `php_network_gethostbyname`.
    fn php_network_gethostbyname(name: *const c_char) -> *mut libc::hostent;
    /// libc's, which the libc crate does not bind.
    fn inet_pton(family: c_int, text: *const c_char, address: *mut c_void) -> c_int;
    #[cfg(not(target_vendor = "apple"))]
    fn inet_ntop(
        family: c_int,
        address: *const c_void,
        text: *mut c_char,
        size: socklen_t,
    ) -> *const c_char;
}

/// The socket a helper works on.
#[derive(Clone, Copy)]
struct Sock {
    socket: Co,
    fd: c_int,
    domain: c_int,
    #[cfg(target_vendor = "apple")]
    kind: c_int,
}

impl Sock {
    fn of(socket: Co) -> Self {
        let (domain, kind) = socket.family();
        #[cfg(not(target_vendor = "apple"))]
        let _ = kind;
        Self {
            socket,
            fd: fd(socket),
            domain,
            #[cfg(target_vendor = "apple")]
            kind,
        }
    }
}

/// A C struct, all zero (as upstream's `= {0}` and `memset`).
fn zeroed<T: Copy>() -> T {
    // SAFETY: only used for libc's plain socket structs, valid all zero.
    unsafe { MaybeUninit::<T>::zeroed().assume_init() }
}

/// `php_error_docref(NULL, E_WARNING, "%s", message)`.
fn warn(message: &[u8]) -> Result<(), Stop> {
    let message = text(message);
    #[expect(clippy::cast_possible_wrap, reason = "E_WARNING is 2")]
    // SAFETY: from a method handler.
    bailout::defer(unsafe { php_error(E_WARNING as i32, &message) });
    checked()
}

/// `PHP_SWOOLE_SOCKET_ERROR`: the socket's error code, and the warning
/// unless the error only means "try again".
fn socket_error(socket: Co, message: &str, error: c_int) -> Result<(), Stop> {
    socket.set_err_code(error);
    if error == libc::EAGAIN || error == libc::EWOULDBLOCK || error == libc::EINPROGRESS {
        return Ok(());
    }
    // SAFETY: strerror returns a NUL-terminated message.
    let reason = unsafe { libc::strerror(error) };
    // SAFETY: as above, read before any other strerror call.
    let reason = unsafe { CStr::from_ptr(reason) };
    warn(format!("{message} [{error}]: {}", reason.to_string_lossy()).as_bytes())
}

/// `h_errno`, as `-10000 - h_errno` reports a host lookup failure.
fn host_error() -> c_int {
    #[cfg(target_os = "linux")]
    {
        unsafe extern "C" {
            fn __h_errno_location() -> *mut c_int;
        }
        // SAFETY: glibc's per-thread slot.
        let slot = unsafe { __h_errno_location() };
        // SAFETY: a live slot.
        -10000 - unsafe { *slot }
    }
    #[cfg(not(target_os = "linux"))]
    {
        unsafe extern "C" {
            static h_errno: c_int;
        }
        // SAFETY: libc's resolver error.
        -10000 - unsafe { h_errno }
    }
}

fn socklen<T>() -> socklen_t {
    socklen_t::try_from(size_of::<T>()).unwrap_or(0)
}

/// `setsockopt(fd, level, name, value, sizeof value)`.
fn setsockopt<T>(fd: c_int, level: c_int, name: c_int, value: &T) -> c_int {
    // SAFETY: `value` is readable for its size.
    unsafe { libc::setsockopt(fd, level, name, ptr::from_ref(value).cast(), socklen::<T>()) }
}

/// `dosockopt:`: set it, or report the socket error.
fn apply<T>(s: Sock, level: c_int, name: c_int, value: &T) -> Result<c_int, Stop> {
    if setsockopt(s.fd, level, name, value) != 0 {
        socket_error(s.socket, "unable to set socket option", errno())?;
        return Ok(FAILURE);
    }
    Ok(SUCCESS)
}

/// `php_string_to_if_index`.
fn index_by_name(name: &[u8]) -> Result<Option<c_uint>, Stop> {
    let name = text(name);
    // SAFETY: a NUL-terminated name.
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        let message = [
            b"no interface with name \"",
            name.as_bytes(),
            b"\" could be found",
        ]
        .concat();
        warn(&message)?;
        return Ok(None);
    }
    Ok(Some(index))
}

/// `php_get_if_index_from_zval`.
fn interface(value: &Value) -> Result<Option<c_uint>, Stop> {
    if let Some(index) = value.as_long() {
        if let Ok(index) = c_uint::try_from(index) {
            return Ok(Some(index));
        }
        let max = c_uint::MAX;
        warn(
            format!("the interface index cannot be negative or larger than {max}; given {index}")
                .as_bytes(),
        )?;
        return Ok(None);
    }
    index_by_name(&string(value)?)
}

/// `php_set_inet_addr`.
fn inet4(s: Sock, host: &[u8]) -> Result<Option<in_addr>, Stop> {
    let host = text(host);
    let mut address = in_addr { s_addr: 0 };
    // SAFETY: NUL-terminated text and a writable address.
    if unsafe { inet_pton(libc::AF_INET, host.as_ptr(), (&raw mut address).cast()) } != 0 {
        return Ok(Some(address));
    }
    let entry = if host.count_bytes() > MAXFQDNLEN {
        ptr::null_mut()
    } else {
        // SAFETY: NUL-terminated text.
        unsafe { php_network_gethostbyname(host.as_ptr()) }
    };
    if entry.is_null() {
        socket_error(s.socket, "Host lookup failed", host_error())?;
        return Ok(None);
    }
    // SAFETY: a live entry, until the next lookup.
    let entry = unsafe { &*entry };
    if entry.h_addrtype != libc::AF_INET {
        warn(b"Host lookup failed: Non AF_INET domain returned on AF_INET socket")?;
        return Ok(None);
    }
    // SAFETY: an AF_INET entry's first address holds an in_addr.
    let first = unsafe { *entry.h_addr_list };
    // SAFETY: as above.
    Ok(Some(unsafe { first.cast::<in_addr>().read_unaligned() }))
}

/// `php_set_inet6_addr`: the address, then any `%scope`.
fn inet6(s: Sock, host: &[u8]) -> Result<Option<sockaddr_in6>, Stop> {
    let host = text(host);
    let mut out: sockaddr_in6 = zeroed();
    let mut address: in6_addr = zeroed();
    // SAFETY: NUL-terminated text and a writable address.
    if unsafe { inet_pton(libc::AF_INET6, host.as_ptr(), (&raw mut address).cast()) } != 0 {
        out.sin6_addr = address;
    } else {
        let Some(address) = resolve6(s, &host)? else {
            return Ok(None);
        };
        out.sin6_addr = address;
    }
    let bytes = host.as_bytes();
    if let Some(at) = bytes.iter().position(|&b| b == b'%') {
        let scope = bytes.get(at + 1..).unwrap_or_default();
        let (mut lval, mut dval) = (0, 0.0);
        // SAFETY: the scope's bytes and writable outputs.
        let kind = unsafe {
            swr_zend_is_numeric_string(
                scope.as_ptr().cast(),
                scope.len(),
                &raw mut lval,
                &raw mut dval,
            )
        };
        let mut scope_id = 0;
        if u32::from(kind) == IS_LONG {
            if lval > 0 {
                scope_id = c_uint::try_from(lval).unwrap_or(0);
            }
        } else if let Some(index) = index_by_name(scope)? {
            scope_id = index;
        }
        out.sin6_scope_id = scope_id;
    }
    Ok(Some(out))
}

/// `getaddrinfo(host, AF_INET6, AI_V4MAPPED | AI_ADDRCONFIG)`'s first address.
fn resolve6(s: Sock, host: &CStr) -> Result<Option<in6_addr>, Stop> {
    let mut hints: libc::addrinfo = zeroed();
    hints.ai_family = libc::AF_INET6;
    hints.ai_flags = libc::AI_V4MAPPED | libc::AI_ADDRCONFIG;
    let mut info = ptr::null_mut();
    // SAFETY: NUL-terminated text, live hints and a writable list.
    unsafe { libc::getaddrinfo(host.as_ptr(), ptr::null(), &raw const hints, &raw mut info) };
    if info.is_null() {
        socket_error(s.socket, "Host lookup failed", host_error())?;
        return Ok(None);
    }
    // SAFETY: the list getaddrinfo returned.
    let first = unsafe { &*info };
    let address = (first.ai_family == libc::PF_INET6
        && first.ai_addrlen == socklen::<sockaddr_in6>())
    // SAFETY: an AF_INET6 entry's address is a sockaddr_in6.
    .then(|| unsafe { first.ai_addr.cast::<sockaddr_in6>().read_unaligned() }.sin6_addr);
    // SAFETY: the list, no longer read.
    unsafe { libc::freeaddrinfo(info) };
    if address.is_none() {
        warn(b"Host lookup failed: Non AF_INET6 domain returned on AF_INET6 socket")?;
    }
    Ok(address)
}

/// `php_set_inet46_addr`: an address of the socket's domain.
fn inet46(s: Sock, host: &[u8]) -> Result<Option<libc::sockaddr_storage>, Stop> {
    let mut storage: libc::sockaddr_storage = zeroed();
    let target = ptr::from_mut(&mut storage);
    if s.domain == libc::AF_INET {
        let Some(address) = inet4(s, host)? else {
            return Ok(None);
        };
        let mut sin: sockaddr_in = zeroed();
        sin.sin_addr = address;
        // SAFETY: a sockaddr_storage holds any sockaddr.
        unsafe { target.cast::<sockaddr_in>().write(sin) };
        storage.ss_family = family(libc::AF_INET);
    } else if s.domain == libc::AF_INET6 {
        let Some(sin6) = inet6(s, host)? else {
            return Ok(None);
        };
        // SAFETY: as above.
        unsafe { target.cast::<sockaddr_in6>().write(sin6) };
        storage.ss_family = family(libc::AF_INET6);
    } else {
        warn(b"IP address used in the context of an unexpected type of socket")?;
        return Ok(None);
    }
    Ok(Some(storage))
}

fn family(domain: c_int) -> libc::sa_family_t {
    libc::sa_family_t::try_from(domain).unwrap_or(0)
}

/// `php_get_address_from_array`.
fn address(s: Sock, table: &Array, key: &CStr) -> Result<Option<libc::sockaddr_storage>, Stop> {
    let Some(value) = table.find(key.to_bytes()) else {
        warn(&[b"no key \"", key.to_bytes(), b"\" passed in optval"].concat())?;
        return Ok(None);
    };
    inet46(s, &string(value)?)
}

/// `php_do_mcast_opt`: a membership change named by `name`.
fn membership(
    s: Sock,
    level: c_int,
    name: c_int,
    source: bool,
    value: &Value,
) -> Result<c_int, Stop> {
    // SAFETY: the live argument, converted in place as upstream does; nothing
    // borrows what it held before.
    let table = unsafe { swr_zend_convert_to_array(value.as_ptr()) };
    // SAFETY: the argument's array now, live for the call.
    let Some(table) = (unsafe { Array::from_ptr(table) }) else {
        return Ok(FAILURE);
    };
    let Some(group) = address(s, table, c"group")? else {
        return Ok(FAILURE);
    };
    let source = if source {
        let Some(source) = address(s, table, c"source")? else {
            return Ok(FAILURE);
        };
        Some(source)
    } else {
        None
    };
    let mut index = 0;
    if let Some(value) = table.find(b"interface") {
        let Some(found) = interface(value)? else {
            return Ok(FAILURE);
        };
        index = found;
    }
    let result = change(s, level, name, &group, source.as_ref(), index)?;
    if result != 0 {
        if result != REPORTED {
            socket_error(s.socket, "unable to set socket option", errno())?;
        }
        return Ok(FAILURE);
    }
    Ok(SUCCESS)
}

/// The RFC 3678 requests: `name` is the request itself.
#[cfg(not(target_vendor = "apple"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "shares the signature of the Apple variant, which can fail"
)]
fn change(
    s: Sock,
    level: c_int,
    name: c_int,
    group: &libc::sockaddr_storage,
    source: Option<&libc::sockaddr_storage>,
    index: c_uint,
) -> Result<c_int, Stop> {
    Ok(if let Some(source) = source {
        let mut request: libc::group_source_req = zeroed();
        request.gsr_interface = index;
        request.gsr_group = *group;
        request.gsr_source = *source;
        setsockopt(s.fd, level, name, &request)
    } else {
        let mut request: libc::group_req = zeroed();
        request.gr_interface = index;
        request.gr_group = *group;
        setsockopt(s.fd, level, name, &request)
    })
}

/// `IP_ADD_MEMBERSHIP` and `IP_DROP_MEMBERSHIP`; upstream picks the request
/// by the socket's type.
#[cfg(target_vendor = "apple")]
fn change(
    s: Sock,
    level: c_int,
    name: c_int,
    group: &libc::sockaddr_storage,
    _source: Option<&libc::sockaddr_storage>,
    index: c_uint,
) -> Result<c_int, Stop> {
    let join = name == libc::IP_ADD_MEMBERSHIP;
    let group = ptr::from_ref(group);
    if s.kind == libc::AF_INET {
        let interface = if index == 0 {
            in_addr {
                s_addr: libc::INADDR_ANY.to_be(),
            }
        } else {
            let Some(address) = interface_address(s, index)? else {
                return Ok(REPORTED);
            };
            address
        };
        let request = libc::ip_mreq {
            // SAFETY: the group as the sockaddr_in upstream reads.
            imr_multiaddr: unsafe { group.cast::<sockaddr_in>().read() }.sin_addr,
            imr_interface: interface,
        };
        let name = if join {
            libc::IP_ADD_MEMBERSHIP
        } else {
            libc::IP_DROP_MEMBERSHIP
        };
        Ok(setsockopt(s.fd, level, name, &request))
    } else if s.kind == libc::AF_INET6 {
        let request = libc::ipv6_mreq {
            // SAFETY: as above, as a sockaddr_in6.
            ipv6mr_multiaddr: unsafe { group.cast::<sockaddr_in6>().read() }.sin6_addr,
            ipv6mr_interface: index,
        };
        let name = if join {
            libc::IPV6_JOIN_GROUP
        } else {
            libc::IPV6_LEAVE_GROUP
        };
        Ok(setsockopt(s.fd, level, name, &request))
    } else {
        let option = if join {
            "MCAST_JOIN_GROUP"
        } else {
            "MCAST_LEAVE_GROUP"
        };
        warn(format!("Option {option} is inapplicable to this socket type").as_bytes())?;
        Ok(REPORTED)
    }
}

/// Linux's request numbers, typed for its `ioctl` (`c_int` on musl, `c_ulong` on glibc).
#[cfg(target_os = "linux")]
const SIOCGIFNAME: libc::Ioctl = 0x8910;
#[cfg(target_os = "linux")]
const SIOCGIFADDR: libc::Ioctl = 0x8915;
#[cfg(not(target_os = "linux"))]
use libc::SIOCGIFADDR;

/// `SIOCGIFNAME`: the interface's name into `request`.
#[cfg(target_os = "linux")]
fn interface_name(fd: c_int, index: c_uint, request: &mut libc::ifreq) -> bool {
    request.ifr_ifru.ifru_ifindex = c_int::try_from(index).unwrap_or(-1);
    // SAFETY: a writable request.
    unsafe { libc::ioctl(fd, SIOCGIFNAME, ptr::from_mut(request)) != -1 }
}

/// `if_indextoname`: the interface's name into `request`.
#[cfg(not(target_os = "linux"))]
fn interface_name(_fd: c_int, index: c_uint, request: &mut libc::ifreq) -> bool {
    // SAFETY: the request's IFNAMSIZ name.
    !unsafe { libc::if_indextoname(index, request.ifr_name.as_mut_ptr()) }.is_null()
}

/// `php_if_index_to_addr4`: the interface's IPv4 address (any for 0).
fn interface_address(s: Sock, index: c_uint) -> Result<Option<in_addr>, Stop> {
    if index == 0 {
        return Ok(Some(in_addr {
            s_addr: libc::INADDR_ANY,
        }));
    }
    let mut request: libc::ifreq = zeroed();
    let named = interface_name(s.fd, index, &mut request);
    // SAFETY: a writable request.
    if !named || unsafe { libc::ioctl(s.fd, SIOCGIFADDR, &raw mut request) } == -1 {
        let error = errno();
        warn(format!("Failed obtaining address for interface {index}: error {error}").as_bytes())?;
        return Ok(None);
    }
    // SAFETY: SIOCGIFADDR wrote an AF_INET sockaddr.
    let address = unsafe {
        (&raw const request.ifr_ifru.ifru_addr)
            .cast::<sockaddr_in>()
            .read_unaligned()
    };
    Ok(Some(address.sin_addr))
}

/// `php_add4_to_if_index`: the index of the interface holding `address` (0
/// for any), for `getOption(IPPROTO_IP, IP_MULTICAST_IF)`.
pub(crate) fn interface_index(address: in_addr) -> Result<Option<c_uint>, Stop> {
    if address.s_addr == libc::INADDR_ANY {
        return Ok(Some(0));
    }
    let mut list = ptr::null_mut();
    // SAFETY: a writable list.
    if unsafe { libc::getifaddrs(&raw mut list) } == -1 {
        warn(format!("Failed obtaining interfaces list: error {}", errno()).as_bytes())?;
        return Ok(None);
    }
    let mut found = None;
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: a live entry of the list.
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: a live address.
        if c_int::from(unsafe { (*entry.ifa_addr).sa_family }) != libc::AF_INET {
            continue;
        }
        // SAFETY: an AF_INET address is a sockaddr_in.
        let held = unsafe { entry.ifa_addr.cast::<sockaddr_in>().read_unaligned() }.sin_addr;
        if held.s_addr == address.s_addr {
            // SAFETY: the entry's NUL-terminated name.
            let index = unsafe { libc::if_nametoindex(entry.ifa_name) };
            found = Some((index != 0).then_some(index).ok_or_else(errno));
            break;
        }
    }
    // SAFETY: the list, no longer read.
    unsafe { libc::freeifaddrs(list) };
    match found {
        Some(Ok(index)) => Ok(Some(index)),
        Some(Err(error)) => {
            warn(format!("Error converting interface name to index: error {error}").as_bytes())?;
            Ok(None)
        }
        None => {
            let shown = std::net::Ipv4Addr::from(u32::from_be(address.s_addr));
            warn(format!("The interface with IP address {shown} was not found").as_bytes())?;
            Ok(None)
        }
    }
}

/// `php_do_setsockopt_ip_mcast`.
pub(crate) fn set_ip(socket: Co, level: c_int, name: c_int, value: &Value) -> Result<c_int, Stop> {
    let s = Sock::of(socket);
    if let Some(&(_, source)) = MEMBERSHIP.iter().find(|(option, _)| *option == name) {
        return membership(s, level, name, source, value);
    }
    match name {
        libc::IP_MULTICAST_IF => {
            let Some(index) = interface(value)? else {
                return Ok(FAILURE);
            };
            let Some(address) = interface_address(s, index)? else {
                return Ok(FAILURE);
            };
            apply(s, level, name, &address)
        }
        libc::IP_MULTICAST_LOOP => apply(s, level, name, &u8::from(value.truthy())),
        libc::IP_MULTICAST_TTL => {
            let Ok(ttl) = u8::try_from(long(value)?) else {
                warn(b"Expected a value between 0 and 255")?;
                return Ok(FAILURE);
            };
            apply(s, level, name, &ttl)
        }
        _ => Ok(UNHANDLED),
    }
}

/// `php_do_setsockopt_ipv6_mcast`, then `php_do_setsockopt_ipv6_rfc3542` for
/// what it leaves.
pub(crate) fn set_ipv6(
    socket: Co,
    level: c_int,
    name: c_int,
    value: &Value,
) -> Result<c_int, Stop> {
    let s = Sock::of(socket);
    if let Some(&(_, source)) = MEMBERSHIP.iter().find(|(option, _)| *option == name) {
        return membership(s, level, name, source, value);
    }
    match name {
        libc::IPV6_MULTICAST_IF => {
            let Some(index) = interface(value)? else {
                return Ok(FAILURE);
            };
            apply(s, level, name, &index)
        }
        libc::IPV6_MULTICAST_LOOP => apply(s, level, name, &c_int::from(value.truthy())),
        libc::IPV6_MULTICAST_HOPS => {
            let hops = long(value)?;
            if !(-1..=255).contains(&hops) {
                warn(b"Expected a value between -1 and 255")?;
                return Ok(FAILURE);
            }
            apply(s, level, name, &c_int::try_from(hops).unwrap_or(-1))
        }
        #[cfg(not(target_vendor = "apple"))]
        _ => set_pktinfo(s, level, name, value),
        #[cfg(target_vendor = "apple")]
        _ => Ok(UNHANDLED),
    }
}

/// `IPV6_PKTINFO` (RFC 3542), which Apple's headers leave out by default.
#[cfg(not(target_vendor = "apple"))]
fn set_pktinfo(s: Sock, level: c_int, name: c_int, value: &Value) -> Result<c_int, Stop> {
    if name != libc::IPV6_PKTINFO {
        return Ok(UNHANDLED);
    }
    let Some(info) = pktinfo(s, value)? else {
        return Ok(FAILURE);
    };
    apply(s, level, name, &info)
}

/// `from_zval_run_conversions`' error, warned once the conversion stops.
#[cfg(not(target_vendor = "apple"))]
fn conversion_error(path: &str, message: &[u8]) -> Result<Option<libc::in6_pktinfo>, Stop> {
    warn(
        &[
            b"error converting user data (path: ",
            path.as_bytes(),
            b"): ",
            message,
        ]
        .concat(),
    )?;
    Ok(None)
}

/// `from_zval_write_in6_pktinfo`: `["addr" => string, "ifindex" => int|string]`.
#[cfg(not(target_vendor = "apple"))]
fn pktinfo(s: Sock, value: &Value) -> Result<Option<libc::in6_pktinfo>, Stop> {
    let Some(table) = value.array() else {
        return conversion_error("in6_pktinfo", b"expected an array here");
    };
    let mut info: libc::in6_pktinfo = zeroed();
    let Some(addr) = table.find(b"addr") else {
        return conversion_error("in6_pktinfo", b"The key 'addr' is required");
    };
    let host = string(addr)?;
    let Some(resolved) = inet6(s, &host)? else {
        let host = text(&host);
        let message = [
            b"could not resolve address '",
            host.as_bytes(),
            b"' to get an AF_INET6 address",
        ]
        .concat();
        return conversion_error("in6_pktinfo > addr", &message);
    };
    info.ipi6_addr = resolved.sin6_addr;
    let Some(ifindex) = table.find(b"ifindex") else {
        return conversion_error("in6_pktinfo", b"The key 'ifindex' is required");
    };
    if let Some(index) = ifindex.as_long() {
        let Ok(index) = c_uint::try_from(index) else {
            let max = c_uint::MAX;
            let message = format!(
                "the interface index cannot be negative or larger than {max}; given {index}"
            );
            return conversion_error("in6_pktinfo > ifindex", message.as_bytes());
        };
        info.ipi6_ifindex = index;
    } else {
        let name = text(&string(ifindex)?);
        // SAFETY: a NUL-terminated name.
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if index == 0 {
            let message = [
                b"no interface with name \"",
                name.as_bytes(),
                b"\" could be found",
            ]
            .concat();
            return conversion_error("in6_pktinfo > ifindex", &message);
        }
        info.ipi6_ifindex = index;
    }
    Ok(Some(info))
}

/// `php_do_getsockopt_ipv6_rfc3542`: `IPV6_PKTINFO` as
/// `["addr" => string, "ifindex" => int]` in `result`.
#[cfg(not(target_vendor = "apple"))]
pub(crate) fn get_ipv6(
    socket: Co,
    level: c_int,
    name: c_int,
    result: *mut zval,
) -> Result<c_int, Stop> {
    if name != libc::IPV6_PKTINFO {
        return Ok(UNHANDLED);
    }
    let mut info: libc::in6_pktinfo = zeroed();
    if crate::socket_options::getsockopt(fd(socket), level, name, &mut info).is_none() {
        socket_error(socket, "unable to get socket option", errno())?;
        return Ok(FAILURE);
    }
    let mut shown = [0 as c_char; INET6_ADDRSTRLEN];
    let length = socklen_t::try_from(shown.len()).unwrap_or(0);
    // SAFETY: a live address and a buffer of `length` bytes.
    unsafe {
        inet_ntop(
            libc::AF_INET6,
            (&raw const info.ipi6_addr).cast(),
            shown.as_mut_ptr(),
            length,
        )
    };
    // SAFETY: inet_ntop left a NUL-terminated (or all-zero) buffer.
    let shown = unsafe { CStr::from_ptr(shown.as_ptr()) };
    // SAFETY: the return slot.
    unsafe { swoole_rs_zend::value::array_init(result) };
    // SAFETY: an array.
    unsafe { swoole_rs_zend::value::add_string(result, c"addr", shown) };
    // SAFETY: as above.
    unsafe { swoole_rs_zend::value::add_long(result, c"ifindex", i64::from(info.ipi6_ifindex)) };
    Ok(SUCCESS)
}

#[cfg(target_vendor = "apple")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature of the other platforms'"
)]
pub(crate) fn get_ipv6(
    _socket: Co,
    _level: c_int,
    _name: c_int,
    _result: *mut zval,
) -> Result<c_int, Stop> {
    Ok(UNHANDLED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_names_its_source_options() {
        let sources = MEMBERSHIP.iter().filter(|(_, source)| *source).count();
        #[cfg(not(target_vendor = "apple"))]
        assert_eq!(sources, 4, "RFC 3678 has four source requests");
        #[cfg(target_vendor = "apple")]
        assert_eq!(sources, 0, "Apple maps only join and leave");
    }

    #[test]
    fn zeroed_structs_are_zero() {
        let storage: libc::sockaddr_storage = zeroed();
        assert_eq!(storage.ss_family, 0, "zeroed family");
        assert_ne!(family(libc::AF_INET6), 0, "family fits");
    }
}
