//! `swoole::Socks5Proxy`'s methods under their upstream symbols, over the Rust proxy
//! owner and its packet buffer; the header keeps `create`'s C++ allocation and
//! `handshake`'s `std::function` send.
use std::ffi::{c_char, c_int};
use swoole_rs_corebase::CCast;
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

/// `swoole::Socks5Proxy`'s head: `ProxyOwner::prepared_.owner`, which
/// `swr_v1_core_proxy_new` made and the proxy frees last.
#[repr(C)]
pub struct Socks5Proxy<'a> {
    owner: &'a ProxyState,
}

impl<'a> Socks5Proxy<'a> {
    /// The head of a proxy over `owner`.
    #[must_use]
    pub const fn new(owner: &'a ProxyState) -> Self {
        Self { owner }
    }
}

struct Packet {
    size: i64,
    diagnostic: i32,
    last_error: i32,
}

/// Packs negotiate (0), authentication (1) or connect (2) into the owner's packet.
fn packet(proxy: &Socks5Proxy<'_>, action: u32) -> Packet {
    let owner = proxy.owner;
    let fields = owner.fields();
    let mut result = Packet {
        size: -1,
        diagnostic: 0,
        last_error: 0,
    };
    let Some(mut buffer) = owner.packet() else {
        return result;
    };
    let buffer = &mut buffer[..];
    match action {
        0 => {
            if let Some(method) = socks_negotiate(!owner.string(1).is_empty(), buffer) {
                owner.update(|fields| fields.method = method);
                result.size = 3;
            }
        }
        1 => result.size = socks_auth(&owner.string(1), &owner.string(2), buffer),
        2 => {
            // Preserve the initialized prefix even when literal address validation fails.
            if let Some(prefix) = buffer.get_mut(..3) {
                prefix.copy_from_slice(&[5, 1, 0]);
            }
            let host = owner.string_with_nul(3);
            let name = host.split_last().map_or(&[][..], |(_, name)| name);
            let mut address: AddressStorage = swoole_rs_corebase::sys::zeroed();
            let mut length = 0;
            let (inet4, inet6);
            let (kind, address): (u8, &[u8]) = if fields.dns_tunnel != 0 {
                (3, name)
            } else {
                let error = swoole_rs_clients::address::assign(
                    &mut address,
                    &mut length,
                    fields.socket_type,
                    &host,
                    fields.target_port,
                    false,
                );
                if error != 0 {
                    result.diagnostic = 1;
                    result.last_error = error;
                    return result;
                }
                if swoole_rs_reactor::type_is(fields.socket_type, swoole_rs_reactor::CLASS_INET4) {
                    inet4 = address.v4().sin_addr.s_addr.to_ne_bytes();
                    (1, &inet4)
                } else if swoole_rs_reactor::type_is(
                    fields.socket_type,
                    swoole_rs_reactor::CLASS_INET6,
                ) {
                    inet6 = address.v6().sin6_addr.s6_addr;
                    (4, &inet6)
                } else {
                    result.diagnostic = 2;
                    return result;
                }
            };
            let port = fields.target_port.c_cast::<u16>();
            result.size = socks_connect(kind, address, port, buffer);
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
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy22pack_negotiate_requestEv")]
#[must_use]
pub extern "C" fn pack_negotiate_request(proxy: &Socks5Proxy<'_>) -> isize {
    let size = packet(proxy, 0).size;
    isize::try_from(size).unwrap_or(-1)
}

/// `ssize_t Socks5Proxy::pack_auth_request()`.
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy17pack_auth_requestEv")]
#[must_use]
pub extern "C" fn pack_auth_request(proxy: &Socks5Proxy<'_>) -> isize {
    let size = packet(proxy, 1).size;
    isize::try_from(size).unwrap_or(-1)
}

/// `ssize_t Socks5Proxy::pack_connect_request()`, with upstream's notices.
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy20pack_connect_requestEv")]
#[must_use]
pub extern "C" fn pack_connect_request(proxy: &Socks5Proxy<'_>) -> isize {
    let result = packet(proxy, 2);
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
/// `data` lends `length` readable bytes.
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy14handshake_stepEPKcm")]
#[must_use]
pub unsafe extern "C" fn handshake_step(
    proxy: &Socks5Proxy<'_>,
    data: *const u8,
    length: usize,
) -> isize {
    // SAFETY: the caller lends `length` readable bytes.
    let Some(reply) = (unsafe { crate::input(data, length) }) else {
        return -1;
    };
    socks5_step(proxy, reply)
}

/// [`handshake_step`] over a borrowed reply.
#[must_use]
pub fn socks5_step(proxy: &Socks5Proxy<'_>, reply: &[u8]) -> isize {
    let owner = proxy.owner;
    let fields = owner.fields();
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
    // The state advances before packing, as in upstream's member order.
    owner.update(|fields| fields.state = decision.state);
    match decision.action {
        1 => pack_auth_request(proxy),
        2 => pack_connect_request(proxy),
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
    fn error_log(kind: c_int, _: c_int, code: c_int, _: &CStr, _: &CStr) {
        if kind == log::LOG_ERROR {
            log::set_last_error(code);
        }
    }

    #[test]
    fn socks5_dns_tunnel_target_host_max_length() {
        log::swr_v1_core_set_log_host(error_log);
        assert_eq!(admit(0, 0), 1);
        let owner = ProxyState::new(true);
        let texts: [&[u8]; 3] = [b"127.0.0.1", b"", b""];
        assert_eq!(owner.configure(texts, 1080, false, 1).status, 0);
        let p = &Socks5Proxy::new(&owner);
        owner.update(|fields| fields.dns_tunnel = 1);
        let target_host = vec![b'a'; usize::from(u8::MAX)];
        assert_eq!(owner.set_string(3, &target_host), 0);
        owner.update(|fields| fields.target_port = 80);

        assert_eq!(pack_connect_request(p), 262);
        let buf = *owner.packet().unwrap();
        assert_eq!(buf[3], 0x03);
        assert_eq!(buf[4], u8::MAX);
        assert_eq!(&buf[5..5 + target_host.len()], &target_host[..]);
        let port = &buf[5 + target_host.len()..7 + target_host.len()];
        assert_eq!(u16::from_be_bytes([port[0], port[1]]), 80);

        assert_eq!(owner.set_string(3, &[b'a'; 256]), 0);
        assert_eq!(pack_connect_request(p), -1);
        assert_eq!(log::last_error(), HANDSHAKE_FAILED);
    }

    #[test]
    fn handshake_steps_through_auth_and_connect() {
        let owner = ProxyState::new(true);
        let texts: [&[u8]; 3] = [b"proxy", b"user", b"pass"];
        assert_eq!(owner.configure(texts, 1080, true, 1).status, 0);
        assert_eq!(admit(4, 4), 2);
        assert_eq!(admit(251, 0), 0);
        let p = &Socks5Proxy::new(&owner);
        assert_eq!(pack_negotiate_request(p), 3);
        assert_eq!(owner.fields().method, 2);
        owner.update(|fields| fields.state = 1);
        let buffer = |n: isize| owner.packet().unwrap()[..usize::try_from(n).unwrap()].to_vec();
        // Too short, then the wrong method: failures that leave the state alone.
        assert_eq!(unsafe { handshake_step(p, [5u8].as_ptr(), 1) }, -1);
        assert_eq!(unsafe { handshake_step(p, [5u8, 0].as_ptr(), 2) }, -1);
        assert_eq!(owner.fields().state, 1);
        // Username/password chosen: the auth request follows.
        let n = unsafe { handshake_step(p, [5u8, 2].as_ptr(), 2) };
        assert_eq!(buffer(n), b"\x01\x04user\x04pass");
        assert_eq!(owner.fields().state, 2);
        // Authenticated: the connect request names the target through the tunnel.
        owner.update(|fields| fields.target_port = 80);
        let n = unsafe { handshake_step(p, [1u8, 0].as_ptr(), 2) };
        assert_eq!(buffer(n), b"\x05\x01\x00\x03\x00\x00\x50");
        // Connected: ready, nothing to send.
        assert_eq!(unsafe { handshake_step(p, [5u8, 0].as_ptr(), 2) }, 0);
        assert_eq!(owner.fields().state, 4);
        assert_eq!(
            unsafe { CStr::from_ptr(strerror(5)) },
            c"Connection refused by destination host"
        );
    }
}
