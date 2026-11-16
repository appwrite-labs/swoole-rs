//! `swoole_get_local_ip()` and `swoole_get_local_mac()`: the host's non-loopback
//! interface addresses, keyed by interface name.
use std::{
    ffi::{CString, c_int},
    io,
    net::IpAddr,
};
use swoole_rs_zend::{Call, E_WARNING, OwnedArray, handlers, report};

handlers! {
    zif_swoole_get_local_ip => local_ip;
    zif_swoole_get_local_mac => local_mac;
}

/// `php_swoole_sys_error(E_WARNING, what)`: record errno as the last error and
/// warn with its message if Swoole displays errors; a bailout of the
/// warning is deferred.
pub fn sys_warning(what: &[u8]) {
    let code = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    swoole_rs_corebase::log::set_last_error(code);
    if !crate::module::display_errors() {
        return;
    }
    let reason = swoole_rs_corebase::sys::strerror(code);
    let mut text = what.to_vec();
    text.extend_from_slice(format!(", Error: {reason}[{code}]").as_bytes());
    // printf's %s stops at the first NUL.
    text.truncate(
        text.iter()
            .position(|&byte| byte == 0)
            .unwrap_or(text.len()),
    );
    let text = CString::new(text).unwrap_or_default();
    swoole_rs_zend::bailout::defer(report(E_WARNING, &text));
}

use swoole_rs_corebase::sys::Interfaces;

/// `Address::get_addr()` (`inet_ntop`) of a non-loopback IPv4 or IPv6 address.
fn printable(ip: IpAddr) -> Option<Vec<u8>> {
    // IN_IS_ADDR_LOOPBACK: 127.0.0.0/8, and ::1.
    (!ip.is_loopback()).then(|| ip.to_string().into_bytes())
}

fn local_ip(call: &Call) {
    // Upstream lists the interfaces before parsing the arguments.
    let Some(interfaces) = Interfaces::new() else {
        sys_warning(b"getifaddrs() failed");
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
    let mut array = OwnedArray::default();
    let up = libc::IFF_UP.unsigned_abs();
    for entry in interfaces.iter() {
        let Some(address) = entry.address().filter(|_| entry.flags() & up != 0) else {
            continue;
        };
        if i64::from(address.sa_family) != family {
            continue;
        }
        if let Some(address) = entry.ip().and_then(printable) {
            array.insert_bytes(entry.name().to_bytes(), Some(&address));
        }
    }
    call.set_owned(array.into_owned());
}

/// `add_assoc_address`: `"%02X:%02X:%02X:%02X:%02X:%02X"` under the name.
fn add_address(array: &mut OwnedArray, name: &[u8], address: &[u8]) {
    let text = address
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    array.insert_bytes(name, Some(text.as_bytes()));
}

#[cfg(target_os = "linux")]
fn local_mac(call: &Call) {
    use swoole_rs_corebase::sys::{Ifreq, InterfaceRequest, interface_ioctl};

    let Ok(sock) = swoole_rs_corebase::sys::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) else {
        sys_warning(b"socket creation failed");
        call.set_bool(false);
        return;
    };
    // Upstream's `SIOCGIFCONF` list: an entry for each IPv4 address, named by the
    // label its address carries, which `getifaddrs` lists the same way.
    let Some(interfaces) = Interfaces::new() else {
        sys_warning(b"ioctl SIOCGIFCONF failed");
        _ = swoole_rs_corebase::sys::close(sock);
        call.set_bool(false);
        return;
    };
    let mut array = OwnedArray::default();
    let loopback = libc::IFF_LOOPBACK.unsigned_abs();
    let inet = |address: &libc::sockaddr| c_int::from(address.sa_family) == libc::AF_INET;
    for entry in interfaces.iter() {
        // `SIOCGIFFLAGS`' flags are the entry's.
        if !entry.address().is_some_and(inet) || entry.flags() & loopback != 0 {
            continue;
        }
        let mut request = Ifreq::named(entry.name().to_bytes());
        if interface_ioctl(sock, InterfaceRequest::HardwareAddress, &mut request).is_ok() {
            add_address(&mut array, request.name().to_bytes(), &request.sa_data());
        }
    }
    _ = swoole_rs_corebase::sys::close(sock);
    call.set_owned(array.into_owned());
}

#[cfg(target_os = "macos")]
fn local_mac(call: &Call) {
    let Some(interfaces) = Interfaces::new() else {
        sys_warning(b"getifaddrs failed");
        call.set_bool(false);
        return;
    };
    let mut array = OwnedArray::default();
    let loopback = libc::IFF_LOOPBACK.unsigned_abs();
    for entry in interfaces.iter() {
        let Some(address) = entry.address().filter(|_| entry.flags() & loopback == 0) else {
            continue;
        };
        if c_int::from(address.sa_family) != libc::AF_LINK {
            continue;
        }
        // SAFETY: a BSD address is its `sa_len` bytes, which the list lends with it.
        let link = unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(address).cast::<u8>(),
                usize::from(address.sa_len),
            )
        };
        // A sockaddr_dl's lengths, then LLADDR: the address follows the name in sdl_data.
        let byte = |offset| link.get(offset).copied().map_or(0, usize::from);
        let alen = byte(std::mem::offset_of!(libc::sockaddr_dl, sdl_alen));
        let start = std::mem::offset_of!(libc::sockaddr_dl, sdl_data)
            + byte(std::mem::offset_of!(libc::sockaddr_dl, sdl_nlen));
        if alen == 6
            && let Some(address) = link.get(start..start + 6)
        {
            add_address(&mut array, entry.name().to_bytes(), address);
        }
    }
    call.set_owned(array.into_owned());
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn local_mac(call: &Call) {
    swoole_rs_zend::bailout::defer(report(
        E_WARNING,
        c"swoole_get_local_mac is not supported on this platform",
    ));
    call.set_bool(false);
}
