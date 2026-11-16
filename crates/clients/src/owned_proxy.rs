//! `swoole::Socks5Proxy`'s methods over the Rust proxy owner and its packet buffer.
//! crates/hooks calls them by path, and [`crate::client`] through the host crates/ffi
//! registers.
use std::ffi::{CStr, c_int};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log::LogLevel::SW_LOG_NOTICE as NOTICE;
use swoole_rs_corebase::{ProxyState, log, net_types::AddressStorage};
use swoole_rs_protocols::{
    ConnectRefusal, SocksConfig, SocksError, SocksSend, socks_auth, socks_config, socks_connect,
    socks_negotiate, socks_strerror, socks_transition,
};

/// `SW_ERROR_SOCKS5_*`.
const UNSUPPORT_VERSION: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SOCKS5_UNSUPPORT_VERSION as c_int;
const UNSUPPORT_METHOD: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SOCKS5_UNSUPPORT_METHOD as c_int;
const AUTH_FAILED: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SOCKS5_AUTH_FAILED as c_int;
const SERVER_ERROR: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SOCKS5_SERVER_ERROR as c_int;
const HANDSHAKE_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SOCKS5_HANDSHAKE_FAILED as c_int;

/// A `swoole::Socks5Proxy`: a borrow of its Rust proxy state.
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

/// The request a packet holds.
#[derive(Clone, Copy)]
enum Request {
    Negotiate,
    Auth,
    Connect,
}

#[derive(Default)]
struct Packet {
    size: Option<usize>,
    /// Upstream's notice for a refused connect request.
    notice: Option<&'static str>,
    last_error: i32,
}

/// Packs `request` into the owner's packet.
fn packet(proxy: &Socks5Proxy<'_>, request: Request) -> Packet {
    let owner = proxy.owner;
    owner
        .with_packet(|buffer| pack(owner, request, buffer))
        .unwrap_or_default()
}

/// Packs `request` into `buffer`, the owner's packet.
fn pack(owner: &ProxyState, request: Request, buffer: &mut [u8]) -> Packet {
    let fields = owner.fields();
    let mut result = Packet::default();
    match request {
        Request::Negotiate => {
            if let Some(method) = socks_negotiate(!owner.string(1).is_empty(), buffer) {
                owner.update(|fields| fields.method = method);
                result.size = Some(3);
            }
        }
        Request::Auth => result.size = socks_auth(&owner.string(1), &owner.string(2), buffer),
        Request::Connect => {
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
                let assigned = crate::address::assign(
                    &mut address,
                    &mut length,
                    fields.socket_type,
                    &host,
                    fields.target_port,
                    false,
                );
                if let Err(error) = assigned {
                    result.notice = Some(
                        "When disable SOCKS5 proxy DNS tunnel connection, the destination host must be an IP address.",
                    );
                    result.last_error = error;
                    return result;
                }
                let kind = swoole_rs_corebase::constants::SocketType::from_raw(fields.socket_type);
                if kind.is_some_and(swoole_rs_corebase::constants::SocketType::is_inet4) {
                    inet4 = address.v4().sin_addr.s_addr.to_ne_bytes();
                    (1, &inet4)
                } else if kind.is_some_and(swoole_rs_corebase::constants::SocketType::is_inet6) {
                    inet6 = address.v6().sin6_addr.s6_addr;
                    (4, &inet6)
                } else {
                    result.notice = Some("Unsupported socket type for SOCKS5");
                    return result;
                }
            };
            let port = fields.target_port.c_cast::<u16>();
            match socks_connect(kind, address, port, buffer) {
                Ok(size) => result.size = Some(size),
                Err(ConnectRefusal::DomainTooLong) => {
                    result.notice = Some("SOCKS5 target host is too long, max length is 255 bytes");
                }
                Err(ConnectRefusal::Invalid) => {}
            }
        }
    }
    result
}

/// `ssize_t Socks5Proxy::pack_negotiate_request()`: the packet's size, or `None`.
#[must_use]
pub fn pack_negotiate_request(proxy: &Socks5Proxy<'_>) -> Option<usize> {
    packet(proxy, Request::Negotiate).size
}

/// `ssize_t Socks5Proxy::pack_auth_request()`: the packet's size, or `None`.
#[must_use]
pub fn pack_auth_request(proxy: &Socks5Proxy<'_>) -> Option<usize> {
    packet(proxy, Request::Auth).size
}

/// `ssize_t Socks5Proxy::pack_connect_request()`, with upstream's notices: the
/// packet's size, or `None`.
#[must_use]
pub fn pack_connect_request(proxy: &Socks5Proxy<'_>) -> Option<usize> {
    let result = packet(proxy, Request::Connect);
    if result.last_error != 0 {
        log::set_last_error(result.last_error);
    }
    if let Some(message) = result.notice {
        log::error_log(
            NOTICE,
            HANDSHAKE_FAILED,
            c"Socks5Proxy::pack_connect_request",
            format_args!("{message}"),
        );
    }
    result.size
}

/// `Socks5Proxy::handshake_step(data, length)` over a borrowed reply: applies it (the
/// state advances unless it is an error), logs a failure as upstream's `handshake`
/// did, and packs the request that answers it. The answering packet's size, 0 when
/// none is due, `None` on failure.
#[must_use]
pub fn socks5_step(proxy: &Socks5Proxy<'_>, reply: &[u8]) -> Option<usize> {
    let owner = proxy.owner;
    let fields = owner.fields();
    let notice = |code, message: std::fmt::Arguments<'_>| {
        log::error_log(NOTICE, code, c"Socks5Proxy::handshake", message);
    };
    let (state, send) = match socks_transition(fields.state, fields.method, reply) {
        Ok(step) => step,
        Err(error) => {
            match error {
                SocksError::Short => notice(
                    HANDSHAKE_FAILED,
                    format_args!("SOCKS5 handshake failed, data length is too short"),
                ),
                SocksError::Version => notice(
                    UNSUPPORT_VERSION,
                    format_args!("SOCKS version is not supported"),
                ),
                SocksError::Method => notice(
                    UNSUPPORT_METHOD,
                    format_args!("SOCKS authentication method is not supported"),
                ),
                SocksError::Auth => notice(
                    AUTH_FAILED,
                    format_args!("SOCKS username/password authentication failed"),
                ),
                SocksError::Server(reply) => notice(
                    SERVER_ERROR,
                    format_args!(
                        "Socks5 server error, reason :{}",
                        socks_strerror(i32::from(reply)).to_string_lossy()
                    ),
                ),
            }
            return None;
        }
    };
    // The state advances before packing, as in upstream's member order.
    owner.update(|fields| fields.state = state);
    match send {
        Some(SocksSend::Auth) => pack_auth_request(proxy),
        Some(SocksSend::Connect) => pack_connect_request(proxy),
        None => Some(0),
    }
}

/// `const char *Socks5Proxy::strerror(int)`.
#[must_use]
pub fn strerror(code: c_int) -> &'static CStr {
    socks_strerror(code)
}

/// `int Socks5Proxy::admit(size_t, size_t)`: `create`'s credential check, with
/// upstream's notice when one is too long.
#[must_use]
pub fn admit(user_length: usize, password_length: usize) -> c_int {
    let config = socks_config(user_length, password_length);
    if config == SocksConfig::Refused {
        log::error_log(
            NOTICE,
            AUTH_FAILED,
            c"Socks5Proxy::create",
            format_args!("SOCKS5 username or password is too long, max length is 250 bytes"),
        );
    }
    // Upstream's codes: 0 refused, 1 anonymous, 2 both credentials.
    match config {
        SocksConfig::Refused => 0,
        SocksConfig::Anonymous => 1,
        SocksConfig::Credentials => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // core-tests/src/protocol/base.cpp's protocol.socks5_strerror.
    #[test]
    fn socks5_strerror() {
        for i in 0..16 {
            let error = strerror(i);
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
    fn error_log(kind: log::LogKind, _: c_int, code: c_int, _: &CStr, _: &CStr) {
        if kind == log::LogKind::Error {
            log::set_last_error(code);
        }
    }

    #[test]
    fn socks5_dns_tunnel_target_host_max_length() {
        log::swr_v1_core_set_log_host(error_log);
        assert_eq!(admit(0, 0), 1);
        let owner = ProxyState::new(true);
        let texts: [&[u8]; 3] = [b"127.0.0.1", b"", b""];
        assert_eq!(owner.configure(texts, 1080, false, 1), Ok(()));
        let p = &Socks5Proxy::new(&owner);
        owner.update(|fields| fields.dns_tunnel = 1);
        let target_host = vec![b'a'; usize::from(u8::MAX)];
        assert!(owner.set_string(3, &target_host));
        owner.update(|fields| fields.target_port = 80);

        assert_eq!(pack_connect_request(p).map_or(-1, usize::cast_signed), 262);
        let buf = owner.with_packet(|packet| *packet).unwrap();
        assert_eq!(buf[3], 0x03);
        assert_eq!(buf[4], u8::MAX);
        assert_eq!(&buf[5..5 + target_host.len()], &target_host[..]);
        let port = &buf[5 + target_host.len()..7 + target_host.len()];
        assert_eq!(u16::from_be_bytes([port[0], port[1]]), 80);

        assert!(owner.set_string(3, &[b'a'; 256]));
        assert_eq!(pack_connect_request(p).map_or(-1, usize::cast_signed), -1);
        assert_eq!(log::last_error(), HANDSHAKE_FAILED);
    }

    #[test]
    fn handshake_steps_through_auth_and_connect() {
        let owner = ProxyState::new(true);
        let texts: [&[u8]; 3] = [b"proxy", b"user", b"pass"];
        assert_eq!(owner.configure(texts, 1080, true, 1), Ok(()));
        assert_eq!(admit(4, 4), 2);
        assert_eq!(admit(251, 0), 0);
        let p = &Socks5Proxy::new(&owner);
        assert_eq!(pack_negotiate_request(p), Some(3));
        assert_eq!(owner.fields().method, 2);
        owner.update(|fields| fields.state = 1);
        let buffer = |n: Option<usize>| {
            owner
                .with_packet(|packet| packet[..n.unwrap()].to_vec())
                .unwrap()
        };
        // Too short, then the wrong method: failures that leave the state alone.
        assert_eq!(socks5_step(p, &[5]), None);
        assert_eq!(socks5_step(p, &[5, 0]), None);
        // An empty reply is too short.
        assert_eq!(socks5_step(p, &[]), None);
        assert_eq!(owner.fields().state, 1);
        // Username/password chosen: the auth request follows.
        let n = socks5_step(p, &[5, 2]);
        assert_eq!(buffer(n), b"\x01\x04user\x04pass");
        assert_eq!(owner.fields().state, 2);
        // Authenticated: the connect request names the target through the tunnel.
        owner.update(|fields| fields.target_port = 80);
        let n = socks5_step(p, &[1, 0]);
        assert_eq!(buffer(n), b"\x05\x01\x00\x03\x00\x00\x50");
        // Connected: ready, nothing to send.
        assert_eq!(socks5_step(p, &[5, 0]), Some(0));
        assert_eq!(owner.fields().state, 4);
        assert_eq!(strerror(5), c"Connection refused by destination host");
    }
}
