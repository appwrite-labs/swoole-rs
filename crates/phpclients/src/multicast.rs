//! `Coroutine\Socket::setOption` and `getOption` at `IPPROTO_IP` and
//! `IPPROTO_IPV6`: multicast membership, interface, TTL, hops and loop, and
//! `IPV6_PKTINFO`, as the ext/sockets helpers upstream copies
//! (`thirdparty/php/sockets`) handle them. Each returns a [`Handled`]: `None` for an
//! option it leaves to the caller, else whether it succeeded (upstream's `SUCCESS`
//! or `FAILURE`), and `Err(Stop)` once a warning's handler bailed out.
use crate::{
    native::Co,
    socket_options::{Stop, checked, errno, fd, long, string, text},
};
use libc::{c_int, c_uint, in_addr, in6_addr, sockaddr_in, sockaddr_in6, socklen_t};
use std::{
    ffi::CStr,
    mem::size_of,
    net::{Ipv4Addr, Ipv6Addr},
    ptr,
    str::FromStr,
};
use swoole_rs_corebase::net_types::AddressStorage;
use swoole_rs_corebase::sys::{InterfaceRequest, Text, interface_ioctl, setsockopt, zeroed};
use swoole_rs_zend::{
    Array, Call, E_WARNING, Value, bailout, report,
    sys::{IS_LONG, swr_zend_convert_to_array},
};

/// An option's result: `None` leaves it to the caller, else whether it succeeded.
pub(crate) type Handled = Result<Option<bool>, Stop>;
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

/// `getaddrinfo`'s hints: an `addrinfo` whose pointers are null, as POSIX requires
/// of every hint but the family, socket type, protocol and flags. Only
/// [`Hints::v6`] makes one.
#[repr(transparent)]
struct Hints(libc::addrinfo);

impl Hints {
    /// `AF_INET6`, with `AI_V4MAPPED | AI_ADDRCONFIG`.
    fn v6() -> Self {
        let mut hints: libc::addrinfo = zeroed();
        hints.ai_family = libc::AF_INET6;
        hints.ai_flags = libc::AI_V4MAPPED | libc::AI_ADDRCONFIG;
        Self(hints)
    }
}

/// The head of a list `getaddrinfo` stored, not yet freed. Only `getaddrinfo`
/// makes one (nothing here constructs it) and only `freeaddrinfo` takes it, by
/// value, so each list is freed at most once.
#[repr(transparent)]
struct AddressList(ptr::NonNull<libc::addrinfo>);

impl AddressList {
    /// The list's first entry.
    fn first(&self) -> &libc::addrinfo {
        // SAFETY: a list getaddrinfo stored, freed only by `freeaddrinfo`, which
        // takes the handle this borrows.
        unsafe { self.0.as_ref() }
    }
}

unsafe extern "C" {
    /// PHP's `php_network_gethostbyname`.
    fn php_network_gethostbyname(name: *const libc::c_char) -> *mut libc::hostent;
}

// libc's. Each reads its `Text`s and hints, writes only the one object a reference
// names and reads or writes no other caller memory, so they are declared `safe`.
unsafe extern "C" {
    /// Resolves `host` with no service under `hints`, storing a new list (none
    /// for an empty one) when it returns 0.
    safe fn getaddrinfo(
        host: Text<'_>,
        service: Option<Text<'_>>,
        hints: &Hints,
        list: &mut Option<AddressList>,
    ) -> c_int;
    /// Frees a list `getaddrinfo` stored, which the handle's move hands over.
    safe fn freeaddrinfo(list: AddressList);
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

/// `php_error_docref(NULL, E_WARNING, "%s", message)`.
fn warn(message: &[u8]) -> Result<(), Stop> {
    let message = text(message);
    bailout::defer(report(E_WARNING, &message));
    checked()
}

/// `PHP_SWOOLE_SOCKET_ERROR`: the socket's error code, and the warning
/// unless the error only means "try again".
fn socket_error(socket: Co, message: &str, error: c_int) -> Result<(), Stop> {
    socket.set_err_code(error);
    if error == libc::EAGAIN || error == libc::EWOULDBLOCK || error == libc::EINPROGRESS {
        return Ok(());
    }
    let reason = crate::socket::strerror(error);
    warn(format!("{message} [{error}]: {}", reason.to_string_lossy()).as_bytes())
}

/// `h_errno`, as `-10000 - h_errno` reports a host lookup failure.
fn host_error() -> c_int {
    -10000 - swoole_rs_corebase::sys::h_errno()
}

fn socklen<T>() -> socklen_t {
    socklen_t::try_from(size_of::<T>()).unwrap_or(0)
}

/// `dosockopt:`: set it, or report the socket error.
fn apply<T: swoole_rs_corebase::sys::Pod>(
    s: Sock,
    level: c_int,
    name: c_int,
    value: &T,
) -> Handled {
    if setsockopt(s.fd, level, name, value) != 0 {
        socket_error(s.socket, "unable to set socket option", errno())?;
        return Ok(Some(false));
    }
    Ok(Some(true))
}

/// `inet_pton`: the address `text` spells in full, as dotted quads or IPv6
/// groups (no `%scope`).
fn parse<T: FromStr>(text: &CStr) -> Option<T> {
    text.to_str().ok()?.parse().ok()
}

/// `php_string_to_if_index`.
fn index_by_name(name: &[u8]) -> Result<Option<c_uint>, Stop> {
    let name = text(name);
    let index = swoole_rs_corebase::sys::if_nametoindex(&name);
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
    if let Some(address) = parse::<Ipv4Addr>(&host) {
        return Ok(Some(in_addr {
            s_addr: u32::from_ne_bytes(address.octets()),
        }));
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
    if let Some(address) = parse::<Ipv6Addr>(&host) {
        out.sin6_addr.s6_addr = address.octets();
    } else {
        let Some(address) = resolve6(s, &host)? else {
            return Ok(None);
        };
        out.sin6_addr = address;
    }
    let bytes = host.as_bytes();
    if let Some(at) = bytes.iter().position(|&b| b == b'%') {
        let scope = bytes.get(at + 1..).unwrap_or_default();
        let (kind, lval, _) = swoole_rs_zend::numeric(scope);
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
    let mut list = None;
    let found = getaddrinfo(host.into(), None, &Hints::v6(), &mut list) == 0;
    let Some(list) = list.filter(|_| found) else {
        socket_error(s.socket, "Host lookup failed", host_error())?;
        return Ok(None);
    };
    let first = list.first();
    let address = (first.ai_family == libc::PF_INET6
        && first.ai_addrlen == socklen::<sockaddr_in6>())
    // SAFETY: an AF_INET6 entry's address is a sockaddr_in6.
    .then(|| unsafe { first.ai_addr.cast::<sockaddr_in6>().read_unaligned() }.sin6_addr);
    freeaddrinfo(list);
    if address.is_none() {
        warn(b"Host lookup failed: Non AF_INET6 domain returned on AF_INET6 socket")?;
    }
    Ok(address)
}

/// `php_set_inet46_addr`: an address of the socket's domain, with its family, in
/// zeroed storage.
fn inet46(s: Sock, host: &[u8]) -> Result<Option<AddressStorage>, Stop> {
    let mut storage: AddressStorage = zeroed();
    if s.domain == libc::AF_INET {
        let Some(address) = inet4(s, host)? else {
            return Ok(None);
        };
        let mut sin: sockaddr_in = zeroed();
        sin.sin_addr = address;
        sin.sin_family = family(libc::AF_INET);
        storage.inet_v4 = sin;
    } else if s.domain == libc::AF_INET6 {
        let Some(mut sin6) = inet6(s, host)? else {
            return Ok(None);
        };
        sin6.sin6_family = family(libc::AF_INET6);
        storage.inet_v6 = sin6;
    } else {
        warn(b"IP address used in the context of an unexpected type of socket")?;
        return Ok(None);
    }
    Ok(Some(storage))
}

/// `address` in a zeroed `sockaddr_storage`, as the RFC 3678 requests carry it.
#[cfg(not(target_vendor = "apple"))]
fn stored(address: &AddressStorage) -> libc::sockaddr_storage {
    const {
        assert!(
            size_of::<AddressStorage>() <= size_of::<libc::sockaddr_storage>(),
            "a sockaddr fits"
        );
    }
    let mut storage: libc::sockaddr_storage = zeroed();
    let at = ptr::from_mut(&mut storage).cast::<AddressStorage>();
    // SAFETY: a sockaddr_storage has room for the address, as checked above, at any
    // alignment.
    unsafe { at.write_unaligned(*address) };
    storage
}

fn family(domain: c_int) -> libc::sa_family_t {
    libc::sa_family_t::try_from(domain).unwrap_or(0)
}

/// `php_get_address_from_array`.
fn address(s: Sock, table: &Array, key: &CStr) -> Result<Option<AddressStorage>, Stop> {
    let Some(value) = table.find(key.to_bytes()) else {
        warn(&[b"no key \"", key.to_bytes(), b"\" passed in optval"].concat())?;
        return Ok(None);
    };
    inet46(s, &string(value)?)
}

/// `php_do_mcast_opt`: a membership change named by `name`.
fn membership(s: Sock, level: c_int, name: c_int, source: bool, value: &Value) -> Handled {
    // SAFETY: the live argument, converted in place as upstream does; nothing
    // borrows what it held before.
    unsafe { swr_zend_convert_to_array(value.as_ptr()) };
    // The argument's array now, live for the call.
    let Some(table) = value.array() else {
        return Ok(Some(false));
    };
    let Some(group) = address(s, table, c"group")? else {
        return Ok(Some(false));
    };
    let source = if source {
        let Some(source) = address(s, table, c"source")? else {
            return Ok(Some(false));
        };
        Some(source)
    } else {
        None
    };
    let mut index = 0;
    if let Some(value) = table.find(b"interface") {
        let Some(found) = interface(value)? else {
            return Ok(Some(false));
        };
        index = found;
    }
    // `None`: failed, and the warning is already out.
    let changed = change(s, level, name, &group, source.as_ref(), index)?;
    if changed == Some(false) {
        socket_error(s.socket, "unable to set socket option", errno())?;
    }
    Ok(Some(changed == Some(true)))
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
    group: &AddressStorage,
    source: Option<&AddressStorage>,
    index: c_uint,
) -> Result<Option<bool>, Stop> {
    let result = if let Some(source) = source {
        let mut request: libc::group_source_req = zeroed();
        request.gsr_interface = index;
        request.gsr_group = stored(group);
        request.gsr_source = stored(source);
        setsockopt(s.fd, level, name, &request)
    } else {
        let mut request: libc::group_req = zeroed();
        request.gr_interface = index;
        request.gr_group = stored(group);
        setsockopt(s.fd, level, name, &request)
    };
    Ok(Some(result == 0))
}

/// `IP_ADD_MEMBERSHIP` and `IP_DROP_MEMBERSHIP`; upstream picks the request
/// by the socket's type.
#[cfg(target_vendor = "apple")]
fn change(
    s: Sock,
    level: c_int,
    name: c_int,
    group: &AddressStorage,
    _source: Option<&AddressStorage>,
    index: c_uint,
) -> Result<Option<bool>, Stop> {
    let join = name == libc::IP_ADD_MEMBERSHIP;
    if s.kind == libc::AF_INET {
        let interface = if index == 0 {
            in_addr {
                s_addr: libc::INADDR_ANY.to_be(),
            }
        } else {
            let Some(address) = interface_address(s, index)? else {
                return Ok(None);
            };
            address
        };
        let request = libc::ip_mreq {
            // The group as the sockaddr_in upstream reads, whatever the domain stored.
            imr_multiaddr: group.v4().sin_addr,
            imr_interface: interface,
        };
        let name = if join {
            libc::IP_ADD_MEMBERSHIP
        } else {
            libc::IP_DROP_MEMBERSHIP
        };
        Ok(Some(setsockopt(s.fd, level, name, &request) == 0))
    } else if s.kind == libc::AF_INET6 {
        let request = libc::ipv6_mreq {
            // As above, as a sockaddr_in6.
            ipv6mr_multiaddr: group.v6().sin6_addr,
            ipv6mr_interface: index,
        };
        let name = if join {
            libc::IPV6_JOIN_GROUP
        } else {
            libc::IPV6_LEAVE_GROUP
        };
        Ok(Some(setsockopt(s.fd, level, name, &request) == 0))
    } else {
        let option = if join {
            "MCAST_JOIN_GROUP"
        } else {
            "MCAST_LEAVE_GROUP"
        };
        warn(format!("Option {option} is inapplicable to this socket type").as_bytes())?;
        Ok(None)
    }
}

/// `SIOCGIFNAME`: the interface's name into `request`.
#[cfg(target_os = "linux")]
fn interface_name(fd: c_int, index: c_uint, request: &mut libc::ifreq) -> bool {
    request.ifr_ifru.ifru_ifindex = c_int::try_from(index).unwrap_or(-1);
    interface_ioctl(fd, InterfaceRequest::Name, request) != -1
}

/// `if_indextoname`: the interface's name into `request`.
#[cfg(not(target_os = "linux"))]
fn interface_name(_fd: c_int, index: c_uint, request: &mut libc::ifreq) -> bool {
    // libc's own declaration: it writes at most `IF_NAMESIZE` bytes, the
    // name's size, and returns the name or null.
    unsafe extern "C" {
        safe fn if_indextoname(
            index: c_uint,
            name: &mut [libc::c_char; libc::IF_NAMESIZE],
        ) -> *mut libc::c_char;
    }
    !if_indextoname(index, &mut request.ifr_name).is_null()
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
    if !named || interface_ioctl(s.fd, InterfaceRequest::Address, &mut request) == -1 {
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
    let Some(list) = swoole_rs_corebase::sys::Interfaces::new() else {
        warn(format!("Failed obtaining interfaces list: error {}", errno()).as_bytes())?;
        return Ok(None);
    };
    let found = list
        .iter()
        .find(|entry| {
            matches!(entry.ip(), Some(std::net::IpAddr::V4(held))
                if u32::from_ne_bytes(held.octets()) == address.s_addr)
        })
        .map(|entry| {
            let index = swoole_rs_corebase::sys::if_nametoindex(entry.name());
            (index != 0).then_some(index).ok_or_else(errno)
        });
    drop(list);
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
pub(crate) fn set_ip(socket: Co, level: c_int, name: c_int, value: &Value) -> Handled {
    let s = Sock::of(socket);
    if let Some(&(_, source)) = MEMBERSHIP.iter().find(|(option, _)| *option == name) {
        return membership(s, level, name, source, value);
    }
    match name {
        libc::IP_MULTICAST_IF => {
            let Some(index) = interface(value)? else {
                return Ok(Some(false));
            };
            let Some(address) = interface_address(s, index)? else {
                return Ok(Some(false));
            };
            apply(s, level, name, &address)
        }
        libc::IP_MULTICAST_LOOP => apply(s, level, name, &u8::from(value.truthy())),
        libc::IP_MULTICAST_TTL => {
            let Ok(ttl) = u8::try_from(long(value)?) else {
                warn(b"Expected a value between 0 and 255")?;
                return Ok(Some(false));
            };
            apply(s, level, name, &ttl)
        }
        _ => Ok(None),
    }
}

/// `php_do_setsockopt_ipv6_mcast`, then `php_do_setsockopt_ipv6_rfc3542` for
/// what it leaves.
pub(crate) fn set_ipv6(socket: Co, level: c_int, name: c_int, value: &Value) -> Handled {
    let s = Sock::of(socket);
    if let Some(&(_, source)) = MEMBERSHIP.iter().find(|(option, _)| *option == name) {
        return membership(s, level, name, source, value);
    }
    match name {
        libc::IPV6_MULTICAST_IF => {
            let Some(index) = interface(value)? else {
                return Ok(Some(false));
            };
            apply(s, level, name, &index)
        }
        libc::IPV6_MULTICAST_LOOP => apply(s, level, name, &c_int::from(value.truthy())),
        libc::IPV6_MULTICAST_HOPS => {
            let hops = long(value)?;
            if !(-1..=255).contains(&hops) {
                warn(b"Expected a value between -1 and 255")?;
                return Ok(Some(false));
            }
            apply(s, level, name, &c_int::try_from(hops).unwrap_or(-1))
        }
        #[cfg(not(target_vendor = "apple"))]
        _ => set_pktinfo(s, level, name, value),
        #[cfg(target_vendor = "apple")]
        _ => Ok(None),
    }
}

/// `IPV6_PKTINFO` (RFC 3542), which Apple's headers leave out by default.
#[cfg(not(target_vendor = "apple"))]
fn set_pktinfo(s: Sock, level: c_int, name: c_int, value: &Value) -> Handled {
    if name != libc::IPV6_PKTINFO {
        return Ok(None);
    }
    let Some(info) = pktinfo(s, value)? else {
        return Ok(Some(false));
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
        let index = swoole_rs_corebase::sys::if_nametoindex(&name);
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
/// `["addr" => string, "ifindex" => int]`, returned by `call`.
#[cfg(not(target_vendor = "apple"))]
pub(crate) fn get_ipv6(call: &Call, socket: Co, level: c_int, name: c_int) -> Handled {
    if name != libc::IPV6_PKTINFO {
        return Ok(None);
    }
    let mut info: libc::in6_pktinfo = zeroed();
    if crate::socket_options::getsockopt(fd(socket), level, name, &mut info).is_none() {
        socket_error(socket, "unable to get socket option", errno())?;
        return Ok(Some(false));
    }
    let shown = inet6_text(Ipv6Addr::from(info.ipi6_addr.s6_addr));
    let mut result = swoole_rs_zend::OwnedArray::default();
    result.insert_bytes(b"addr", Some(shown.as_bytes()));
    result.insert_long(b"ifindex", i64::from(info.ipi6_ifindex));
    call.set_owned(result.into_owned());
    Ok(Some(true))
}

/// `inet_ntop(AF_INET6)`: RFC 5952 text, with the IPv4-compatible and
/// IPv4-mapped forms ending in a dotted quad.
#[cfg(not(target_vendor = "apple"))]
fn inet6_text(address: Ipv6Addr) -> String {
    let groups = address.segments();
    let [.., a, b, c, d] = address.octets();
    // glibc embeds the IPv4 form only when the leading zero run is exactly six groups.
    let compatible = groups[..6] == [0; 6] && groups[6] != 0;
    if compatible {
        format!("::{}", Ipv4Addr::new(a, b, c, d))
    } else {
        address.to_string()
    }
}

#[cfg(target_vendor = "apple")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature of the other platforms'"
)]
pub(crate) fn get_ipv6(_call: &Call, _socket: Co, _level: c_int, _name: c_int) -> Handled {
    Ok(None)
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

    #[cfg(not(target_vendor = "apple"))]
    unsafe extern "C" {
        fn inet_pton(
            family: c_int,
            text: *const libc::c_char,
            address: *mut std::ffi::c_void,
        ) -> c_int;
        fn inet_ntop(
            family: c_int,
            address: *const std::ffi::c_void,
            text: *mut libc::c_char,
            size: socklen_t,
        ) -> *const libc::c_char;
    }

    #[cfg(not(target_vendor = "apple"))]
    const TEXTS: &[&CStr] = &[
        c"1.2.3.4",
        c"255.255.255.255",
        c"01.2.3.4",
        c"1.2.3",
        c"1.2.3.4 ",
        c"256.1.1.1",
        c"::",
        c"::1",
        c"::ffff:1.2.3.4",
        c"fe80::1%eth0",
        c"2001:db8::1",
        c"1:2:3:4:5:6:7:8:9",
        c"",
    ];

    /// glibc's, the reference: Apple's libc also takes octets with leading
    /// zeros and a `%scope` suffix, which [`parse`] refuses on every platform.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn parse_matches_inet_pton() {
        for text in TEXTS {
            let mut v4 = [0_u8; 4];
            let ok = unsafe { inet_pton(libc::AF_INET, text.as_ptr(), v4.as_mut_ptr().cast()) };
            assert_eq!(
                parse::<Ipv4Addr>(text).map(|a| a.octets()),
                (ok == 1).then_some(v4),
                "{text:?} as IPv4"
            );
            let mut v6 = [0_u8; 16];
            let ok = unsafe { inet_pton(libc::AF_INET6, text.as_ptr(), v6.as_mut_ptr().cast()) };
            assert_eq!(
                parse::<Ipv6Addr>(text).map(|a| a.octets()),
                (ok == 1).then_some(v6),
                "{text:?} as IPv6"
            );
        }
    }

    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn inet6_text_matches_inet_ntop() {
        let addresses = [
            "::",
            "::1",
            "::2",
            "::1.2.3.4",
            "::0.1.0.0",
            "::ffff:1.2.3.4",
            "::1:0:0:0",
            "fe80::1",
            "2001:db8::1:0:0:1",
            "1:0:0:2:0:0:0:3",
            "1::",
            "1:2:3:4:5:6:7:8",
        ];
        for address in addresses {
            let address: Ipv6Addr = address.parse().unwrap();
            let octets = address.octets();
            let mut shown: [libc::c_char; 46] = [0; 46];
            let length = socklen_t::try_from(shown.len()).unwrap();
            let text = unsafe {
                inet_ntop(
                    libc::AF_INET6,
                    octets.as_ptr().cast(),
                    shown.as_mut_ptr(),
                    length,
                )
            };
            let text = unsafe { CStr::from_ptr(text) }.to_str().unwrap();
            assert_eq!(inet6_text(address), text, "{address:?}");
        }
    }
}
