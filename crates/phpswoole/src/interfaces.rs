//! `swoole_get_local_ip()` and `swoole_get_local_mac()`: the host's non-loopback
//! interface addresses, keyed by interface name.
use crate::ops;
use std::{
    ffi::{CStr, c_char, c_int},
    io, ptr,
};
use swoole_rs_zend::{Call, E_WARNING, handlers, message, php_error, set_key};

handlers! {
    zif_swoole_get_local_ip => local_ip;
    zif_swoole_get_local_mac => local_mac;
}

/// `php_swoole_sys_error(E_WARNING, what)`: record errno as the last error and
/// warn with its message if Swoole displays errors.
fn sys_warning(what: &str) {
    let code = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // SAFETY: C++'s setter.
    unsafe { (ops().set_last_error)(code) };
    // SAFETY: C++'s getter.
    if unsafe { (ops().display_errors)() } == 0 {
        return;
    }
    // SAFETY: libc's message table.
    let reason = unsafe { libc::strerror(code) };
    // SAFETY: a NUL-terminated message.
    let reason = unsafe { CStr::from_ptr(reason) };
    let text = message!("{what}, Error: {}[{code}]", reason.to_string_lossy());
    // SAFETY: a handler, with a NUL-terminated message.
    unsafe { php_error(E_WARNING, text.as_cstr()) };
}

/// The host's interfaces (`getifaddrs`), freed on drop.
struct Interfaces(*mut libc::ifaddrs);

impl Interfaces {
    fn new() -> Option<Self> {
        let mut list = ptr::null_mut();
        // SAFETY: a writable out-pointer.
        (unsafe { libc::getifaddrs(&raw mut list) } == 0).then_some(Self(list))
    }

    fn iter(&self) -> impl Iterator<Item = &libc::ifaddrs> {
        // SAFETY: the list lives as long as `self`.
        std::iter::successors(unsafe { self.0.as_ref() }, |entry| {
            // SAFETY: as above.
            unsafe { entry.ifa_next.as_ref() }
        })
    }
}

impl Drop for Interfaces {
    fn drop(&mut self) {
        // SAFETY: the list `getifaddrs` returned, freed once.
        unsafe { libc::freeifaddrs(self.0) };
    }
}

/// `ifa_name` as bytes.
fn name(entry: &libc::ifaddrs) -> &[u8] {
    // SAFETY: interface names are NUL-terminated.
    unsafe { CStr::from_ptr(entry.ifa_name) }.to_bytes()
}

unsafe extern "C" {
    /// `<arpa/inet.h>`; not in the libc crate.
    fn inet_ntop(
        family: c_int,
        source: *const std::ffi::c_void,
        buffer: *mut c_char,
        size: libc::socklen_t,
    ) -> *const c_char;
}

/// `INET6_ADDRSTRLEN`: room for any printable address.
const INET6_ADDRSTRLEN: usize = 46;

/// `Address::get_addr()` (`inet_ntop`) of a non-loopback IPv4 or IPv6 address.
/// # Safety
/// `address` is a live socket address of its family's size.
unsafe fn printable(address: *const libc::sockaddr) -> Option<Vec<u8>> {
    let mut buffer = [0 as c_char; INET6_ADDRSTRLEN];
    // SAFETY: per the caller.
    let family = c_int::from(unsafe { (*address).sa_family });
    let source = match family {
        libc::AF_INET => {
            // SAFETY: an AF_INET address is a sockaddr_in.
            let inet = unsafe { &*address.cast::<libc::sockaddr_in>() };
            // IN_IS_ADDR_LOOPBACK: 127.0.0.0/8.
            if u32::from_be(inet.sin_addr.s_addr) >> 24 == 127 {
                return None;
            }
            (&raw const inet.sin_addr).cast()
        }
        libc::AF_INET6 => {
            // SAFETY: an AF_INET6 address is a sockaddr_in6.
            let inet6 = unsafe { &*address.cast::<libc::sockaddr_in6>() };
            if inet6.sin6_addr.s6_addr == std::net::Ipv6Addr::LOCALHOST.octets() {
                return None;
            }
            (&raw const inet6.sin6_addr).cast()
        }
        _ => return None,
    };
    #[expect(clippy::cast_possible_truncation, reason = "INET6_ADDRSTRLEN is 46")]
    let size = buffer.len() as libc::socklen_t;
    // SAFETY: a source of `family` and a buffer of `size` bytes.
    let text = unsafe { inet_ntop(family, source, buffer.as_mut_ptr(), size) };
    // SAFETY: on success `inet_ntop` wrote a NUL-terminated string into `buffer`.
    (!text.is_null()).then(|| unsafe { CStr::from_ptr(text) }.to_bytes().to_vec())
}

fn local_ip(call: &Call) {
    // Upstream lists the interfaces before parsing the arguments.
    let Some(interfaces) = Interfaces::new() else {
        sys_warning("getifaddrs() failed");
        call.set_bool(false);
        return;
    };
    let parsed = call
        .arity(0, 1)
        .and_then(|()| call.long(1, i64::from(libc::AF_INET)));
    let Ok(family) = parsed else {
        call.set_bool(false);
        return;
    };
    call.set_array();
    let up = libc::IFF_UP.unsigned_abs();
    for entry in interfaces.iter() {
        if entry.ifa_addr.is_null() || entry.ifa_flags & up == 0 {
            continue;
        }
        // SAFETY: a live address.
        if i64::from(unsafe { (*entry.ifa_addr).sa_family }) != family {
            continue;
        }
        // SAFETY: a live address of its family's size.
        if let Some(address) = unsafe { printable(entry.ifa_addr) } {
            // SAFETY: the return slot holds an array.
            unsafe { set_key(call.return_value(), name(entry), Some(&address)) };
        }
    }
}

/// `add_assoc_address`: `"%02X:%02X:%02X:%02X:%02X:%02X"` under the name.
fn add_address(call: &Call, name: &[u8], address: &[u8]) {
    let text = address
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    // SAFETY: the return slot holds an array.
    unsafe { set_key(call.return_value(), name, Some(text.as_bytes())) };
}

#[cfg(target_os = "linux")]
fn local_mac(call: &Call) {
    use std::mem::{size_of, size_of_val, zeroed};
    // The requests as `ioctl` takes them (an int on musl).
    const SIOCGIFCONF: libc::Ioctl = 0x8912;
    const SIOCGIFFLAGS: libc::Ioctl = 0x8913;
    const SIOCGIFHWADDR: libc::Ioctl = 0x8927;
    // Upstream's 4096-byte buffer, as whole requests.
    const REQUESTS: usize = 4096 / size_of::<libc::ifreq>();

    // SAFETY: plain socket creation.
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if sock < 0 {
        sys_warning("socket creation failed");
        call.set_bool(false);
        return;
    }
    // SAFETY: plain data; all-zero is valid.
    let mut buffer: [libc::ifreq; REQUESTS] = unsafe { zeroed() };
    // SAFETY: as above.
    let mut conf: libc::ifconf = unsafe { zeroed() };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "under 4096 bytes"
    )]
    let length = size_of_val(&buffer) as c_int;
    conf.ifc_len = length;
    conf.ifc_ifcu.ifcu_req = buffer.as_mut_ptr();
    // SAFETY: a socket and an ifconf over a live buffer.
    if unsafe { libc::ioctl(sock, SIOCGIFCONF, &raw mut conf) } < 0 {
        sys_warning("ioctl SIOCGIFCONF failed");
        // SAFETY: our socket.
        unsafe { libc::close(sock) };
        call.set_bool(false);
        return;
    }
    call.set_array();
    let count = usize::try_from(conf.ifc_len).unwrap_or(0) / size_of::<libc::ifreq>();
    for entry in buffer.iter().take(count) {
        // SAFETY: plain data; all-zero is valid.
        let mut request: libc::ifreq = unsafe { zeroed() };
        let end = libc::IFNAMSIZ - 1;
        request.ifr_name[..end].copy_from_slice(&entry.ifr_name[..end]);
        // SAFETY: a socket and a named request.
        if unsafe { libc::ioctl(sock, SIOCGIFFLAGS, &raw mut request) } < 0 {
            continue;
        }
        // SAFETY: SIOCGIFFLAGS filled the flags.
        let flags = c_int::from(unsafe { request.ifr_ifru.ifru_flags });
        if flags & libc::IFF_LOOPBACK != 0 {
            continue;
        }
        // SAFETY: a socket and a named request.
        if unsafe { libc::ioctl(sock, SIOCGIFHWADDR, &raw mut request) } == 0 {
            // SAFETY: SIOCGIFHWADDR filled the hardware address.
            let data = unsafe { request.ifr_ifru.ifru_hwaddr.sa_data };
            let bytes = data.map(|byte| byte.to_ne_bytes()[0]);
            // SAFETY: the name is NUL-terminated within IFNAMSIZ.
            let name = unsafe { CStr::from_ptr(request.ifr_name.as_ptr()) };
            add_address(call, name.to_bytes(), &bytes);
        }
    }
    // SAFETY: our socket.
    unsafe { libc::close(sock) };
}

#[cfg(target_os = "macos")]
fn local_mac(call: &Call) {
    let Some(interfaces) = Interfaces::new() else {
        sys_warning("getifaddrs failed");
        call.set_bool(false);
        return;
    };
    call.set_array();
    let loopback = libc::IFF_LOOPBACK.unsigned_abs();
    for entry in interfaces.iter() {
        if entry.ifa_addr.is_null() || entry.ifa_flags & loopback != 0 {
            continue;
        }
        // SAFETY: a live address.
        if c_int::from(unsafe { (*entry.ifa_addr).sa_family }) != libc::AF_LINK {
            continue;
        }
        // SAFETY: an AF_LINK address is a sockaddr_dl.
        let link = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_dl>() };
        if link.sdl_alen == 6 {
            // LLADDR: the address follows the name in sdl_data.
            let start = usize::from(link.sdl_nlen);
            let base = (&raw const link.sdl_data).cast::<u8>();
            // SAFETY: sdl_data holds the name then sdl_alen address bytes.
            let address = unsafe { base.add(start) };
            // SAFETY: as above.
            let address = unsafe { std::slice::from_raw_parts(address, 6) };
            add_address(call, name(entry), address);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn local_mac(call: &Call) {
    // SAFETY: a handler, with a literal message.
    unsafe {
        php_error(
            E_WARNING,
            c"swoole_get_local_mac is not supported on this platform",
        )
    };
    call.set_bool(false);
}
