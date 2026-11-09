//! `swoole::Socks5Proxy`'s methods under their upstream symbols, over the Rust proxy
//! owner and its packet buffer; the header keeps `create`'s C++ allocation and
//! `handshake`'s `std::function` send.
use std::ffi::{c_char, c_int};
use std::ptr;
use swoole_rs_corebase::{ProxyState, log, net_types::AddressStorage};
use swoole_rs_protocols::{
    socks_auth, socks_config, socks_connect, socks_negotiate, socks_strerror, socks_transition,
};

/// `SW_LOG_NOTICE`.
const NOTICE: c_int = 3;
/// `SW_ERROR_SOCKS5_*`.
const UNSUPPORT_VERSION: c_int = 7001;
const UNSUPPORT_METHOD: c_int = 7002;
const AUTH_FAILED: c_int = 7003;
const SERVER_ERROR: c_int = 7004;
const HANDSHAKE_FAILED: c_int = 7005;

/// `swoole::Socks5Proxy`'s head: `ProxyOwner::prepared_.owner`.
#[repr(C)]
pub struct Socks5Proxy {
    owner: *mut ProxyState,
}

/// The proxy's owner.
/// # Safety
/// `proxy` is a live `Socks5Proxy`; its owner outlives the borrow.
unsafe fn owner<'a>(proxy: *const Socks5Proxy) -> &'a ProxyState {
    // SAFETY: the caller lends a live proxy.
    let owner = unsafe { (*proxy).owner };
    // SAFETY: a live proxy holds its live owner.
    unsafe { &*owner }
}

struct Packet {
    size: i64,
    diagnostic: i32,
    last_error: i32,
}

/// Packs negotiate (0), authentication (1) or connect (2) into the owner's buffer.
/// # Safety
/// `proxy` is a live `Socks5Proxy`, exclusively lent for packet and state writes.
unsafe fn packet(proxy: *const Socks5Proxy, action: u32) -> Packet {
    // SAFETY: as the caller promises.
    let owner = unsafe { owner(proxy) };
    // SAFETY: a scalar snapshot before accessing its distinct packet storage.
    let fields = unsafe { *owner.fields.get() };
    // SAFETY: SOCKS owners contain an exclusive512-byte packet allocation.
    let buffer = unsafe { owner.buffer() };
    // SAFETY: as above, lent to this call alone.
    let buffer = unsafe { std::slice::from_raw_parts_mut(buffer, 512) };
    let mut result = Packet {
        size: -1,
        diagnostic: 0,
        last_error: 0,
    };
    match action {
        0 => {
            // SAFETY: the stable username slot is unchanged while packing.
            let user = unsafe { owner.string(1) };
            if let Some(method) = socks_negotiate(user.length != 0, buffer) {
                // SAFETY: the owner's method field is exclusively lent to negotiation.
                unsafe { (*owner.fields.get()).method = method };
                result.size = 3;
            }
        }
        1 => {
            // SAFETY: the owner's credentials are stable and disjoint from packet storage.
            let user = unsafe { owner.string(1) };
            // SAFETY: the password slot remains unchanged while packing.
            let password = unsafe { owner.string(2) };
            // SAFETY: the stable username span, disjoint from the packet storage.
            let user = unsafe { crate::input(user.data.cast(), user.length) };
            // SAFETY: as above, for the password.
            let password = unsafe { crate::input(password.data.cast(), password.length) };
            if let (Some(user), Some(password)) = (user, password) {
                result.size = socks_auth(user, password, buffer);
            }
        }
        2 => {
            // Preserve the initialized prefix even when literal address validation fails.
            if let Some(prefix) = buffer.get_mut(..3) {
                prefix.copy_from_slice(&[5, 1, 0]);
            }
            // SAFETY: the target slot stays stable through address parsing and packing.
            let host = unsafe { owner.string(3) };
            // SAFETY: every member of this POD sockaddr union admits zero bytes.
            let mut address: AddressStorage = unsafe { std::mem::zeroed() };
            let mut length = 0;
            let (kind, bytes, size) = if fields.dns_tunnel != 0 {
                (3, host.data.cast::<u8>(), host.length)
            } else {
                // SAFETY: the exact address operation receives a local union/length and terminated host.
                let error = unsafe {
                    swoole_rs_clients::address::swr_v1_clients_address_assign(
                        ptr::from_mut(&mut address).cast(),
                        &raw mut length,
                        fields.socket_type,
                        host.data,
                        host.length,
                        fields.target_port,
                        None,
                    )
                };
                if error != 0 {
                    result.diagnostic = 1;
                    result.last_error = error;
                    return result;
                }
                if swoole_rs_reactor::type_is(fields.socket_type, swoole_rs_reactor::CLASS_INET4) {
                    // SAFETY: the preceding address operation initialized the IPv4 union arm.
                    let bytes = unsafe { ptr::addr_of!(address.inet_v4.sin_addr).cast::<u8>() };
                    (1, bytes, size_of::<libc::in_addr>())
                } else if swoole_rs_reactor::type_is(
                    fields.socket_type,
                    swoole_rs_reactor::CLASS_INET6,
                ) {
                    // SAFETY: the preceding address operation initialized the IPv6 union arm.
                    let bytes = unsafe { ptr::addr_of!(address.inet_v6.sin6_addr).cast::<u8>() };
                    (4, bytes, size_of::<libc::in6_addr>())
                } else {
                    result.diagnostic = 2;
                    return result;
                }
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "native SOCKS packet API narrows its int target port to uint16_t"
            )]
            let port = fields.target_port as u16;
            // SAFETY: the selected initialized address is disjoint from the owned packet buffer.
            if let Some(address) = unsafe { crate::input(bytes, size) } {
                result.size = socks_connect(kind, address, port, buffer);
            }
            if result.size == -2 {
                result.size = -1;
                result.diagnostic = 3;
            }
        }
        _ => {}
    }
    result
}

/// `ssize_t Socks5Proxy::pack_negotiate_request()`.
/// # Safety
/// As [`packet`].
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy22pack_negotiate_requestEv")]
pub unsafe extern "C" fn pack_negotiate_request(proxy: *mut Socks5Proxy) -> isize {
    // SAFETY: as the caller promises.
    let size = unsafe { packet(proxy, 0) }.size;
    isize::try_from(size).unwrap_or(-1)
}

/// `ssize_t Socks5Proxy::pack_auth_request()`.
/// # Safety
/// As [`packet`].
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy17pack_auth_requestEv")]
pub unsafe extern "C" fn pack_auth_request(proxy: *mut Socks5Proxy) -> isize {
    // SAFETY: as the caller promises.
    let size = unsafe { packet(proxy, 1) }.size;
    isize::try_from(size).unwrap_or(-1)
}

/// `ssize_t Socks5Proxy::pack_connect_request()`, with upstream's notices.
/// # Safety
/// As [`packet`].
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy20pack_connect_requestEv")]
pub unsafe extern "C" fn pack_connect_request(proxy: *mut Socks5Proxy) -> isize {
    // SAFETY: as the caller promises.
    let result = unsafe { packet(proxy, 2) };
    if result.last_error != 0 {
        log::set_last_error(result.last_error);
    }
    let message = match result.diagnostic {
        1 => {
            "When disable SOCKS5 proxy DNS tunnel connection, the destination host must be an IP address."
        }
        2 => "Unsupported socket type for SOCKS5",
        3 => "SOCKS5 target host is too long, max length is 255 bytes",
        _ => "",
    };
    if !message.is_empty() {
        log::error_log(
            NOTICE,
            HANDSHAKE_FAILED,
            c"Socks5Proxy::pack_connect_request",
            format_args!("{message}"),
        );
    }
    isize::try_from(result.size).unwrap_or(-1)
}

/// `ssize_t Socks5Proxy::handshake_step(const char *, size_t)`: applies a server
/// reply (the state advances unless it is an error), logs a failure as upstream's
/// `handshake` did, and packs the request that answers it. Returns its size, 0 when
/// none is due, -1 on failure.
/// # Safety
/// As [`packet`]; `data` lends `length` readable bytes.
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy14handshake_stepEPKcm")]
pub unsafe extern "C" fn handshake_step(
    proxy: *mut Socks5Proxy,
    data: *const u8,
    length: usize,
) -> isize {
    // SAFETY: the caller lends `length` readable bytes.
    let Some(reply) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    // SAFETY: as the caller promises.
    let owner = unsafe { owner(proxy) };
    // SAFETY: a scalar snapshot; nothing else writes the fields during this call.
    let fields = unsafe { *owner.fields.get() };
    let decision = socks_transition(fields.state, fields.method, reply);
    let notice = |code, message: std::fmt::Arguments<'_>| {
        log::error_log(NOTICE, code, c"Socks5Proxy::handshake", message);
    };
    match decision.error {
        0 => {}
        1 => notice(
            HANDSHAKE_FAILED,
            format_args!("SOCKS5 handshake failed, data length is too short"),
        ),
        2 => notice(
            UNSUPPORT_VERSION,
            format_args!("SOCKS version is not supported"),
        ),
        3 => notice(
            UNSUPPORT_METHOD,
            format_args!("SOCKS authentication method is not supported"),
        ),
        4 => notice(
            AUTH_FAILED,
            format_args!("SOCKS username/password authentication failed"),
        ),
        _ => {
            let reason = socks_strerror(i32::from(decision.reply));
            notice(
                SERVER_ERROR,
                format_args!("Socks5 server error, reason :{}", reason.to_string_lossy()),
            );
        }
    }
    if decision.error != 0 {
        return -1;
    }
    // SAFETY: the state advances before packing, as in upstream's member order.
    unsafe { (*owner.fields.get()).state = decision.state };
    match decision.action {
        // SAFETY: as the caller promises.
        1 => unsafe { pack_auth_request(proxy) },
        // SAFETY: as the caller promises.
        2 => unsafe { pack_connect_request(proxy) },
        _ => 0,
    }
}

/// `const char *Socks5Proxy::strerror(int)`.
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy8strerrorEi")]
#[must_use]
pub extern "C" fn strerror(code: c_int) -> *const c_char {
    socks_strerror(code).as_ptr()
}

/// `int Socks5Proxy::admit(size_t, size_t)`: `create`'s credential check, with
/// upstream's notice when one is too long.
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy5admitEmm")]
#[must_use]
pub extern "C" fn admit(user_length: usize, password_length: usize) -> c_int {
    let config = socks_config(user_length, password_length);
    if config == 0 {
        log::error_log(
            NOTICE,
            AUTH_FAILED,
            c"Socks5Proxy::create",
            format_args!("SOCKS5 username or password is too long, max length is 250 bytes"),
        );
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use swoole_rs_corebase::{
        CoreStringView, swr_v1_core_proxy_configure, swr_v1_core_proxy_new, swr_v1_core_string_set,
    };

    // core-tests/src/protocol/base.cpp's protocol.socks5_strerror.
    #[test]
    fn socks5_strerror() {
        for i in 0..16 {
            // SAFETY: strerror returns a static NUL-terminated string.
            let error = unsafe { CStr::from_ptr(strerror(i)) };
            if i > 0x08 {
                assert_eq!(error, c"Unknown error");
            } else {
                assert!(error.count_bytes() > 5);
            }
        }
    }

    // core-tests/src/protocol/base.cpp's protocol.socks5_dns_tunnel_target_host_max_length:
    // `Socks5Proxy::create(SW_SOCK_TCP, "127.0.0.1", 1080, "", "")`, then a 255- and a
    // 256-byte target host through the tunnel.
    /// Native's `swoole_error_log`, as far as the case observes it: it sets the last error.
    unsafe extern "C" fn error_log(
        kind: c_int,
        _: c_int,
        code: c_int,
        _: *const c_char,
        _: *const c_char,
    ) {
        if kind == log::LOG_ERROR {
            log::set_last_error(code);
        }
    }

    #[test]
    fn socks5_dns_tunnel_target_host_max_length() {
        log::swr_v1_core_set_log_host(error_log);
        assert_eq!(admit(0, 0), 1);
        let prepared = swr_v1_core_proxy_new(true);
        let text = |s: &'static str| CoreStringView {
            data: s.as_ptr().cast(),
            length: s.len(),
        };
        let texts = [text("127.0.0.1"), text(""), text("")];
        let configured =
            unsafe { swr_v1_core_proxy_configure(prepared.owner, texts.as_ptr(), 1080, false, 1) };
        assert_eq!(configured.status, 0);
        let mut proxy = Socks5Proxy {
            owner: prepared.owner,
        };
        let p = &raw mut proxy;
        let set_target_host = |host: &[u8]| unsafe {
            assert_eq!(
                swr_v1_core_string_set(prepared.strings[3], host.as_ptr().cast(), host.len()),
                0
            );
        };
        unsafe { (*prepared.fields).dns_tunnel = 1 };
        let target_host = vec![b'a'; usize::from(u8::MAX)];
        set_target_host(&target_host);
        unsafe { (*prepared.fields).target_port = 80 };

        assert_eq!(unsafe { pack_connect_request(p) }, 262);
        let buf = unsafe { std::slice::from_raw_parts(prepared.buffer, 512) };
        assert_eq!(buf[3], 0x03);
        assert_eq!(buf[4], u8::MAX);
        assert_eq!(&buf[5..5 + target_host.len()], &target_host[..]);
        let port = &buf[5 + target_host.len()..7 + target_host.len()];
        assert_eq!(u16::from_be_bytes([port[0], port[1]]), 80);

        set_target_host(&[b'a'; 256]);
        assert_eq!(unsafe { pack_connect_request(p) }, -1);
        assert_eq!(log::last_error(), HANDSHAKE_FAILED);
        unsafe { swoole_rs_corebase::swr_v1_core_proxy_free(prepared.owner) };
    }

    #[test]
    fn handshake_steps_through_auth_and_connect() {
        let prepared = swr_v1_core_proxy_new(true);
        let proxy = Socks5Proxy {
            owner: prepared.owner,
        };
        let text = |s: &'static str| CoreStringView {
            data: s.as_ptr().cast(),
            length: s.len(),
        };
        let texts = [text("proxy"), text("user"), text("pass")];
        let configured =
            unsafe { swr_v1_core_proxy_configure(prepared.owner, texts.as_ptr(), 1080, true, 1) };
        assert_eq!(configured.status, 0);
        assert_eq!(admit(4, 4), 2);
        assert_eq!(admit(251, 0), 0);
        let p = &raw const proxy;
        let p = p.cast_mut();
        assert_eq!(unsafe { pack_negotiate_request(p) }, 3);
        assert_eq!(unsafe { (*prepared.fields).method }, 2);
        unsafe { (*prepared.fields).state = 1 };
        let buffer = |n: isize| unsafe {
            std::slice::from_raw_parts(prepared.buffer, usize::try_from(n).unwrap()).to_vec()
        };
        // Too short, then the wrong method: failures that leave the state alone.
        assert_eq!(unsafe { handshake_step(p, [5u8].as_ptr(), 1) }, -1);
        assert_eq!(unsafe { handshake_step(p, [5u8, 0].as_ptr(), 2) }, -1);
        assert_eq!(unsafe { (*prepared.fields).state }, 1);
        // Username/password chosen: the auth request follows.
        let n = unsafe { handshake_step(p, [5u8, 2].as_ptr(), 2) };
        assert_eq!(buffer(n), b"\x01\x04user\x04pass");
        assert_eq!(unsafe { (*prepared.fields).state }, 2);
        // Authenticated: the connect request names the target through the tunnel.
        unsafe { (*prepared.fields).target_port = 80 };
        let n = unsafe { handshake_step(p, [1u8, 0].as_ptr(), 2) };
        assert_eq!(buffer(n), b"\x05\x01\x00\x03\x00\x00\x50");
        // Connected: ready, nothing to send.
        assert_eq!(unsafe { handshake_step(p, [5u8, 0].as_ptr(), 2) }, 0);
        assert_eq!(unsafe { (*prepared.fields).state }, 4);
        assert_eq!(
            unsafe { CStr::from_ptr(strerror(5)) },
            c"Connection refused by destination host"
        );
        unsafe { swoole_rs_corebase::swr_v1_core_proxy_free(prepared.owner) };
    }
}
