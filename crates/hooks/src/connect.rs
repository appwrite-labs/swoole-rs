//! The coroutine socket's client sequence, whole: `connect(host)` (TLS host name,
//! proxy target, the socket family switch, the address loop, the connect, the
//! SOCKS5 and HTTP proxy exchanges and the TLS handshake) over the socket's record,
//! its proxies' owners and its resolver context, and `ssl_handshake`'s steps and
//! waits. Native code keeps only the name lookup (`Host::lookup`).
use crate::co_io::{
    Message, NATIVE, available, borrowed_err, default_err, errno, owned_err, set_err,
};
use crate::co_socket::{Co, reinit};
use crate::packet::recv_packet;
use crate::socket_members::assign;
use crate::socket_scope::ProtocolScope;
use core::ffi::{c_char, c_int};
use core::ptr::{from_ref, null, null_mut};

use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::{NameResolverContext, ProxyState, log};
use swoole_rs_http_ffi::{proxy_accept, proxy_connect};
use swoole_rs_protocols_ffi::{Socks5Proxy, pack_negotiate_request};
use swoole_rs_reactor::{CLASS_INET4, CLASS_INET6, type_is};
use swoole_rs_tls::upstream::socket_free;

/// Swoole error codes this sequence sets.
const WRONG_OPERATION: c_int = swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int;
const BAD_HOST_ADDR: c_int = swoole_rs_error::ErrorCode::SW_ERROR_BAD_HOST_ADDR as c_int;
const SSL_BAD_CLIENT: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SSL_BAD_CLIENT as c_int;
const SSL_HANDSHAKE_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SSL_HANDSHAKE_FAILED as c_int;
const SOCKS5_HANDSHAKE_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SOCKS5_HANDSHAKE_FAILED as c_int;
const HTTP_PROXY_HANDSHAKE_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_HTTP_PROXY_HANDSHAKE_FAILED as c_int;
const HTTP_PROXY_BAD_RESPONSE: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_HTTP_PROXY_BAD_RESPONSE as c_int;
/// `SW_SOCK_TCP` and `SW_SOCK_TCP6`.
const SOCK_TCP: c_int = 1;
const SOCK_TCP6: c_int = 3;
/// What [`Connecting::set_host`] stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// The connect target.
    Connect,
    /// The TLS host name (no port).
    Tls,
    /// The proxy's target, which makes the proxy the connect target.
    Proxy,
}
/// The socket's proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proxy {
    Socks5,
    Http,
}
/// `set_err(-1)` is the argument-less `set_err()`: the last error, else errno.
const LAST_ERROR: c_int = -1;
/// `SW_EVENT_RDWR`.
const RDWR: u32 = swoole_rs_corebase::constants::EventType::SW_EVENT_RDWR as u32;
/// `SW_SOCKS5_STATE_HANDSHAKE` and `SW_SOCKS5_STATE_READY`, crates/clients's (a test
/// compares them).
const SOCKS5_HANDSHAKE: u8 = 1;
const SOCKS5_READY: u8 = 4;
/// The extra record's string slots: the connect host and the TLS host name.
const CONNECT_HOST: usize = 1;
const SSL_HOST_NAME: usize = 2;

/// `connect(host)`'s effects on the socket.
pub trait Connecting {
    /// `sock_domain`.
    fn domain(&self) -> c_int;
    /// `AF_INET`/`AF_INET6` for an IPv4/IPv6 socket, else 0.
    fn family(&self) -> c_int;
    fn proxy(&self) -> Option<Proxy>;
    /// Whether the HTTP proxy handshakes.
    fn http_handshake(&self) -> bool;
    /// Whether the socket has an SSL context.
    fn ssl(&self) -> bool;
    /// `is_available(SW_EVENT_RDWR)`.
    fn available(&mut self) -> bool;
    /// `set_err(code)`, or `set_err()` for -1.
    fn set_err(&mut self, code: c_int);
    /// `set_err(code, text)`, or with `swoole_strerror(code)` without text.
    fn set_err_text(&mut self, code: c_int, text: Option<&[u8]>);
    /// `swoole_get_last_error()`.
    fn last_error(&self) -> c_int;
    /// Whether the last error is `SW_ERROR_BAD_HOST_ADDR`.
    fn bad_host(&self) -> bool {
        self.last_error() == BAD_HOST_ADDR
    }
    /// Stores `host` and `port` as `which`.
    fn set_host(&mut self, which: Host, host: &[u8], port: c_int);
    /// The connect target's host.
    fn connect_host(&self) -> Vec<u8>;
    /// `reinit_sock(type)`.
    fn reinit(&mut self, kind: c_int) -> bool;
    /// `Address::assign` of the connect target, without resolving.
    fn assign(&mut self) -> bool;
    /// Resolves the connect target's name into it; false when it found none.
    fn lookup(&mut self) -> bool;
    /// `connect(addr)` of the assigned address.
    fn connect(&mut self) -> bool;
    /// The socket's `send` and `recv`.
    fn send(&mut self, data: &[u8]) -> isize;
    fn recv(&mut self, into: &mut [u8]) -> isize;
    /// Packs the SOCKS5 negotiation, moving the proxy to the handshake state.
    fn socks5_negotiate(&mut self) -> Vec<u8>;
    /// `Socks5Proxy::handshake` of a reply (sending what it packs): whether the
    /// proxy is ready, or none when it failed.
    fn socks5_step(&mut self, reply: &[u8]) -> Option<bool>;
    /// Packs the HTTP CONNECT request into the write buffer, and returns it.
    fn http_request(&mut self) -> Vec<u8>;
    /// Receives the HTTP proxy's response head (`recv_packet` under a "\r\n\r\n"
    /// protocol): its result, and whether `HttpProxy::handshake` accepts it.
    fn http_response(&mut self) -> (isize, bool);
    /// Clears the write buffer, and the read buffer when `received`.
    fn http_done(&mut self, received: bool);
    /// `ssl_is_server = false`.
    fn client(&mut self);
    /// `ssl_handshake()`.
    fn handshake(&mut self) -> bool;
}

/// `Address::verify_ip`: the host's `c_str()` is an IP literal of `family`, as
/// `inet_pton` reads one.
fn is_ip(family: c_int, host: &[u8]) -> bool {
    let text = host.split(|&b| b == 0).next().unwrap_or_default();
    let Ok(text) = std::ffi::CString::new(text) else {
        return false;
    };
    swoole_rs_clients::address::verify_ip(family, &text)
}

/// Why [`resolve`] failed.
#[derive(Debug, PartialEq, Eq)]
enum ResolveError {
    /// An assignment failed other than with a bad host address.
    Assign,
    Lookup,
}

/// The address loop: up to two assignments with a lookup between them. A second
/// bad host address falls through as upstream's does.
fn resolve(ops: &mut impl Connecting) -> Result<(), ResolveError> {
    for _ in 0..2 {
        if ops.assign() {
            break;
        }
        if !ops.bad_host() {
            return Err(ResolveError::Assign);
        }
        if !ops.lookup() {
            return Err(ResolveError::Lookup);
        }
    }
    Ok(())
}

/// `socks5_handshake`: the negotiation, then each reply until the proxy is ready.
fn socks5(ops: &mut impl Connecting) -> bool {
    let request = ops.socks5_negotiate();
    if ops.send(&request) < 0 {
        return false;
    }
    let mut reply = [0u8; 512];
    loop {
        let n = ops.recv(&mut reply);
        let Some(received) = usize::try_from(n).ok().filter(|&n| n > 0) else {
            return false;
        };
        match ops.socks5_step(reply.get(..received).unwrap_or_default()) {
            Some(true) => return true,
            Some(false) => {}
            None => return false,
        }
    }
}

/// `http_proxy_handshake`: CONNECT, then the response head.
fn http(ops: &mut impl Connecting) -> bool {
    let request = ops.http_request();
    if ops.send(&request) != request.len().cast_signed() {
        ops.http_done(false);
        return false;
    }
    let (received, accepted) = ops.http_response();
    if received > 0 && !accepted {
        // Upstream reports the request as the response too.
        let mut text = b"wrong http_proxy response received, \n[Request]: ".to_vec();
        text.extend_from_slice(&request);
        text.extend_from_slice(b"\n[Response]: ");
        text.extend_from_slice(&request);
        ops.set_err_text(HTTP_PROXY_BAD_RESPONSE, Some(&text));
    }
    ops.http_done(true);
    received > 0 && accepted
}

/// `Socket::connect(host, port)` over `ops`.
fn connect_host(ops: &mut impl Connecting, host: &[u8], port: c_int) -> bool {
    if !ops.available() {
        return false;
    }
    let proxy = ops.proxy();
    let proxied = proxy.is_some();
    // A proxy replaces the host, so a TLS host name has to come from it now.
    if ops.ssl() && proxied && !is_ip(ops.domain(), host) {
        ops.set_host(Host::Tls, host, 0);
    }
    ops.set_host(
        if proxied { Host::Proxy } else { Host::Connect },
        host,
        port,
    );
    if proxied {
        let target = ops.connect_host();
        let family = ops.family();
        let switch = if family == libc::AF_INET6 && is_ip(libc::AF_INET, &target) {
            Some(SOCK_TCP)
        } else if family == libc::AF_INET && is_ip(libc::AF_INET6, &target) {
            Some(SOCK_TCP6)
        } else {
            None
        };
        if switch.is_some_and(|kind| !ops.reinit(kind)) {
            return false;
        }
    }
    if resolve(ops).is_err() {
        let error = ops.last_error();
        ops.set_err_text(error, None);
        return false;
    }
    if !ops.connect() {
        return false;
    }
    if proxy == Some(Proxy::Socks5) && !socks5(ops) {
        ops.set_err(SOCKS5_HANDSHAKE_FAILED);
        return false;
    }
    if proxy == Some(Proxy::Http) && ops.http_handshake() && !http(ops) {
        ops.set_err(HTTP_PROXY_HANDSHAKE_FAILED);
        return false;
    }
    ops.client();
    if ops.ssl() && !ops.handshake() {
        if ops.last_error() == 0 {
            ops.set_err(SSL_HANDSHAKE_FAILED);
        }
        return false;
    }
    true
}

/// `std::stoi`'s value of `text`: leading space, a sign and digits; 0 without digits.
fn stoi(text: &[u8]) -> c_int {
    let text = text.trim_ascii_start();
    let (negative, digits) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, text),
    };
    let value = digits
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0i64, |n, d| {
            n.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        });
    c_int::try_from(if negative { -value } else { value }).unwrap_or(0)
}

/// A live native socket's `connect(host)`.
struct Live<'a> {
    socket: Co,
    /// The `NameResolver::Context` the lookup uses.
    resolve: &'a mut NameResolverContext,
    socks5: Option<&'a ProxyState>,
    http: Option<&'a ProxyState>,
    address: SocketAddress,
    resolved: bool,
}

impl Live<'_> {
    /// A failed copy (allocation) leaves the previous string.
    fn set_slot(&self, index: usize, value: &[u8]) {
        self.socket
            .with_extra(|extra| extra.strings.get_mut(index).map(|slot| slot.set(value)));
    }

    fn string(&self, index: usize) -> Vec<u8> {
        self.socket
            .with_extra(|extra| extra.strings.get(index).map(|slot| slot.bytes().to_vec()))
            .flatten()
            .unwrap_or_default()
    }

    /// The SOCKS5 packet's first `length` bytes.
    fn socks5_packet(&self, length: isize) -> Vec<u8> {
        let length = usize::try_from(length).unwrap_or(0).min(512);
        self.socks5
            .and_then(ProxyState::packet)
            .map(|packet| packet.get(..length).unwrap_or_default().to_vec())
            .unwrap_or_default()
    }
}

impl Connecting for Live<'_> {
    fn domain(&self) -> c_int {
        self.socket.sock_domain()
    }

    fn family(&self) -> c_int {
        match self.socket.net(|socket| socket.socket_type) {
            Some(kind) if type_is(kind, CLASS_INET6) => libc::AF_INET6,
            Some(kind) if type_is(kind, CLASS_INET4) => libc::AF_INET,
            _ => 0,
        }
    }

    fn proxy(&self) -> Option<Proxy> {
        match (self.socks5, self.http) {
            (Some(_), _) => Some(Proxy::Socks5),
            (None, Some(_)) => Some(Proxy::Http),
            (None, None) => None,
        }
    }

    fn http_handshake(&self) -> bool {
        self.http.is_some_and(|http| !http.fields().dont_handshake)
    }

    fn ssl(&self) -> bool {
        !self.socket.tls_owner().is_null()
    }

    fn available(&mut self) -> bool {
        available(self.socket, RDWR)
    }

    fn set_err(&mut self, code: c_int) {
        if code == LAST_ERROR {
            default_err(self.socket, log::last_error());
        } else {
            set_err(self.socket, code);
        }
    }

    fn set_err_text(&mut self, code: c_int, text: Option<&[u8]>) {
        match text {
            None => borrowed_err(self.socket, code, Message::Strerror),
            // A failed copy (allocation) leaves the previous message.
            Some(text) => {
                owned_err(self.socket, code, text);
            }
        }
    }

    fn last_error(&self) -> c_int {
        log::last_error()
    }

    fn set_host(&mut self, which: Host, host: &[u8], port: c_int) {
        let (mut host, mut port) = (host.to_vec(), port);
        if which == Host::Proxy
            && let Some(proxy) = self.socks5.or(self.http)
        {
            // A failed copy (allocation) leaves the previous target.
            proxy.set_string(3, &host);
            proxy.update(|fields| fields.target_port = port);
            port = proxy.fields().port;
            host = proxy.string(0).to_vec();
        }
        if which == Host::Tls {
            self.set_slot(SSL_HOST_NAME, &host);
        } else {
            self.set_slot(CONNECT_HOST, &host);
            self.socket.set_connect_port(port);
        }
    }

    fn connect_host(&self) -> Vec<u8> {
        self.string(CONNECT_HOST)
    }

    fn reinit(&mut self, kind: c_int) -> bool {
        let Some(old) = reinit(self.socket, kind) else {
            log::set_last_error(errno());
            return false;
        };
        // SAFETY: the network::Socket the record no longer holds.
        unsafe { socket_free(old) };
        true
    }

    fn assign(&mut self) -> bool {
        let (kind, port) = (self.socket.kind(), self.socket.connect_port());
        let host = self.connect_host();
        assign(&mut self.address, kind, &host, port)
    }

    fn lookup(&mut self) -> bool {
        if !self.resolved {
            self.resolved = true;
            if self.ssl() && self.proxy().is_none() {
                self.set_slot(SSL_HOST_NAME, &self.connect_host());
            }
        }
        let name = self.connect_host();
        let domain = self.socket.sock_domain();
        let timeout = self
            .socket
            .net(|network| network.timeouts.dns)
            .unwrap_or(0.0);
        let mut address = Vec::<u8>::new();
        let co = (NATIVE.current)();
        // Locked like wait_event.
        self.socket.with_state(|state| state.bind(RDWR, co));
        let with_port =
            crate::resolve::socket_lookup(self.resolve, domain, timeout, &name, &mut address);
        self.socket.with_state(|state| state.bind(RDWR, null_mut()));
        if address.is_empty() {
            Connecting::set_err(self, log::last_error());
            return false;
        }
        let delimiter = if domain == libc::AF_INET6 { b'@' } else { b':' };
        match address.iter().position(|&b| b == delimiter) {
            Some(at) if with_port => {
                let (name, port) = address.split_at(at);
                self.socket
                    .set_connect_port(stoi(port.get(1..).unwrap_or_default()));
                self.set_slot(CONNECT_HOST, name);
            }
            _ => self.set_slot(CONNECT_HOST, &address),
        }
        true
    }

    fn connect(&mut self) -> bool {
        self.socket.connect_addr(&self.address)
    }

    fn send(&mut self, data: &[u8]) -> isize {
        self.socket.send(data)
    }

    fn recv(&mut self, into: &mut [u8]) -> isize {
        self.socket.recv(into)
    }

    fn socks5_negotiate(&mut self) -> Vec<u8> {
        let Some(owner) = self.socks5 else {
            return Vec::new();
        };
        let length = pack_negotiate_request(&Socks5Proxy::new(owner));
        owner.update(|fields| fields.state = SOCKS5_HANDSHAKE);
        self.socks5_packet(length)
    }

    fn socks5_step(&mut self, reply: &[u8]) -> Option<bool> {
        let owner = self.socks5?;
        let n = swoole_rs_protocols_ffi::socks5_step(&Socks5Proxy::new(owner), reply);
        if n < 0 || (n > 0 && self.send(&self.socks5_packet(n)) != n) {
            return None;
        }
        Some(owner.fields().state == SOCKS5_READY)
    }

    fn http_request(&mut self) -> Vec<u8> {
        let Some(http) = self.http else {
            return Vec::new();
        };
        let mut name = self.socket.tls_host_name();
        if name.is_empty() {
            name = http.string(3).to_vec();
        }
        self.socket.with_buffer(true, |write| {
            let mut request = vec![0; write.size()];
            let written = proxy_connect(http, &name, &mut request);
            request.truncate(written);
            write.clear();
            write.append(&request);
            request
        })
    }

    fn http_response(&mut self) -> (isize, bool) {
        let scope = ProtocolScope::new(self.socket);
        self.socket.set_open_eof_check(true);
        self.socket.set_open_length_check(false);
        let mut protocol = self.socket.protocol();
        protocol.package_eof_len = 4;
        protocol.package_eof = *b"\r\n\r\n\0\0\0\0";
        self.socket.set_protocol(protocol);
        let n = recv_packet(self.socket, 0.0);
        let accepted = n > 0
            && self
                .socket
                .with_buffer(false, |read| proxy_accept(read.view()));
        drop(scope);
        (n, accepted)
    }

    fn http_done(&mut self, received: bool) {
        for write in [false, true] {
            if write || received {
                self.socket.with_buffer(write, |buffer| {
                    buffer.clear();
                });
            }
        }
    }

    fn client(&mut self) {
        self.socket.set_ssl_is_server(false);
    }

    fn handshake(&mut self) -> bool {
        self.socket.ssl_handshake()
    }
}

impl Co {
    /// `Socket::connect(host, port)`, through the socket's proxies; the lookup uses
    /// the socket's `NameResolver::Context`, or a fresh one. In a coroutine.
    #[must_use]
    pub fn connect_host(self, host: &[u8], port: c_int) -> bool {
        let mut fallback = NameResolverContext::default();
        let context = self.resolve_context().cast::<NameResolverContext>();
        // SAFETY: null or the context its owner set, which outlives the socket's connect.
        let resolve = unsafe { context.as_mut() }.unwrap_or(&mut fallback);
        let proxy = |owner: &Option<Box<ProxyState>>| owner.as_deref().map_or(null(), from_ref);
        let proxies = self.with_extra(|extra| extra.proxies.each_ref().map(proxy));
        let proxies = proxies.unwrap_or_default();
        // SAFETY: each null or the socket's own proxy, which stays put while the socket
        // connects: no `set_proxy` runs meanwhile.
        let [socks5, http] = proxies.map(|owner| unsafe { owner.as_ref() });
        let mut live = Live {
            socket: self,
            resolve,
            socks5,
            http,
            address: SocketAddress::default(),
            resolved: false,
        };
        connect_host(&mut live, host, port)
    }

    /// `set_http_proxy` (`http`) or `set_socks5_proxy`: a new proxy, which replaces
    /// the socket's. A SOCKS5 proxy whose credentials are too long (logged) clears it.
    /// No connect of the socket is running.
    pub fn set_proxy(self, http: bool, host: &[u8], port: c_int, user: &[u8], password: &[u8]) {
        let proxy = if http {
            Some(swoole_rs_http_ffi::http_proxy(host, port, user, password))
        } else {
            match swoole_rs_protocols_ffi::admit(user.len(), password.len()) {
                0 => None,
                config => {
                    let owner = ProxyState::new(true);
                    // An allocation failure leaves the strings it didn't reach empty.
                    _ = owner.configure([host, user, password], port, config == 2, self.kind());
                    Some(owner)
                }
            }
        };
        // The replaced proxy goes with the statement, once the metadata's loan has ended.
        self.with_extra(|extra| {
            let [socks5, http_proxy] = &mut extra.proxies;
            core::mem::replace(if http { http_proxy } else { socks5 }, proxy)
        });
    }

    /// `f` over the socket's HTTP proxy owner; None without one. `f` must not
    /// reach the socket's metadata again (a nested loan panics).
    pub fn with_http_proxy<T>(self, f: impl FnOnce(&ProxyState) -> T) -> Option<T> {
        self.with_extra(|extra| extra.proxies[1].as_deref().map(f))
            .flatten()
    }
}

/// `Socket::connect(host, port)` of the native `socket` (its `this`, a live
/// [`Co`]), for C++.
///
/// # Safety
/// In a coroutine; `host` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_connect_host(
    socket: Co,
    host: *const c_char,
    length: usize,
    port: c_int,
) -> bool {
    // SAFETY: as the caller promises; copied before any effect.
    let host = unsafe { core::slice::from_raw_parts(host.cast::<u8>(), length) }.to_vec();
    socket.connect_host(&host, port)
}

/// How [`Handshaking::step`] drives the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeMode {
    /// `ssl_connect` steps.
    Client,
    /// `ssl_accept` steps.
    Server,
    /// A step that completes or fails by itself (`io_uring`'s memory BIOs).
    SelfDriven,
}

/// What one handshake step left to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The handshake is done.
    Ready,
    /// It needs the peer's next bytes.
    Wait,
    /// It can step again now.
    Again,
    Failed,
}

/// The socket's effects for `ssl_handshake`.
pub trait Handshaking {
    fn mode(&self) -> HandshakeMode;
    fn handshaked(&self) -> bool;
    /// Whether the SSL context has its `SSL_CTX`, and whether it verifies the peer.
    fn context_ready(&self) -> bool;
    fn verify_peer(&self) -> bool;
    /// `is_available(SW_EVENT_RDWR)`.
    fn available(&mut self) -> bool;
    /// `set_err(code)`, or `set_err()` for -1.
    fn set_err(&mut self, code: c_int);
    /// `ssl_context_create()`, and `ssl_create` of the socket's context.
    fn context_create(&mut self) -> bool;
    fn create(&mut self) -> bool;
    fn step(&mut self) -> Step;
    /// Waits to read under the read timeout: a timer of its own when `fresh`,
    /// else one timer for the whole handshake.
    fn wait(&mut self, fresh: bool) -> bool;
    /// `ssl_verify(allow_self_signed)`.
    fn verify(&mut self) -> bool;
}

/// Steps until done. A client waits under a timer per wait and sets the last
/// error when a step fails; a server waits under one timer and sets
/// `SSL_BAD_CLIENT` whenever it fails.
fn steps(ops: &mut impl Handshaking) -> bool {
    let mode = ops.mode();
    loop {
        match ops.step() {
            Step::Ready => return true,
            Step::Again => {}
            Step::Wait if ops.wait(mode == HandshakeMode::Client) => {}
            step => {
                match (mode, step) {
                    (HandshakeMode::Client, Step::Failed) => ops.set_err(LAST_ERROR),
                    (HandshakeMode::Server, _) => ops.set_err(SSL_BAD_CLIENT),
                    _ => {}
                }
                return false;
            }
        }
    }
}

/// `Socket::ssl_handshake`, up to marking the socket handshaked, which native
/// code does when this returns true.
#[must_use]
pub fn ssl_handshake(ops: &mut impl Handshaking) -> bool {
    if ops.handshaked() {
        ops.set_err(WRONG_OPERATION);
        return false;
    }
    let prepared = ops.available() && (ops.context_ready() || ops.context_create()) && ops.create();
    prepared && steps(ops) && (!ops.verify_peer() || ops.verify())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use swoole_rs_clients::client;

    #[test]
    fn socks5_states_are_the_clients() {
        assert_eq!(SOCKS5_HANDSHAKE, client::SOCKS5_HANDSHAKE, "handshake");
        assert_eq!(SOCKS5_READY, client::SOCKS5_READY, "ready");
    }

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// Scripted results, consumed in order per effect name.
        static SCRIPT: RefCell<Vec<(&'static str, isize)>> = const { RefCell::new(Vec::new()) };
        static HOST: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    fn log(entry: String) {
        LOG.with(|log| log.borrow_mut().push(entry));
    }

    /// The next scripted result for `name`, or `default`.
    fn take(name: &'static str, default: isize) -> isize {
        log(name.to_owned());
        SCRIPT.with(|script| {
            let mut script = script.borrow_mut();
            match script.iter().position(|(n, _)| *n == name) {
                Some(i) => script.remove(i).1,
                None => default,
            }
        })
    }

    /// A scripted socket: each effect logs its name and takes its scripted result.
    struct Mock {
        proxy: Option<Proxy>,
        ssl: bool,
        family: c_int,
        http_handshake: bool,
    }

    impl Connecting for Mock {
        fn domain(&self) -> c_int {
            self.family
        }
        fn family(&self) -> c_int {
            self.family
        }
        fn proxy(&self) -> Option<Proxy> {
            self.proxy
        }
        fn http_handshake(&self) -> bool {
            self.http_handshake
        }
        fn ssl(&self) -> bool {
            self.ssl
        }
        fn available(&mut self) -> bool {
            take("available", 1) != 0
        }
        fn set_err(&mut self, code: c_int) {
            log(format!("err {code}"));
        }
        fn set_err_text(&mut self, code: c_int, text: Option<&[u8]>) {
            let text = text.map_or_else(
                || "strerror".to_owned(),
                |text| String::from_utf8_lossy(text).into_owned(),
            );
            log(format!("err {code} {text}"));
        }
        fn last_error(&self) -> c_int {
            c_int::try_from(take("last_error", 0)).unwrap_or(-9)
        }
        fn bad_host(&self) -> bool {
            take("bad_host", 0) != 0
        }
        fn set_host(&mut self, which: Host, h: &[u8], port: c_int) {
            log(format!(
                "host {which:?} {} {port}",
                String::from_utf8_lossy(h)
            ));
            if which != Host::Tls {
                HOST.with(|host| {
                    *host.borrow_mut() = if which == Host::Proxy {
                        b"::1".to_vec()
                    } else {
                        h.to_vec()
                    };
                });
            }
        }
        fn connect_host(&self) -> Vec<u8> {
            HOST.with(|host| host.borrow().clone())
        }
        fn reinit(&mut self, kind: c_int) -> bool {
            take(
                if kind == SOCK_TCP6 {
                    "reinit6"
                } else {
                    "reinit4"
                },
                1,
            ) != 0
        }
        fn assign(&mut self) -> bool {
            take("assign", 1) != 0
        }
        fn lookup(&mut self) -> bool {
            take("lookup", 1) != 0
        }
        fn connect(&mut self) -> bool {
            take("connect", 1) != 0
        }
        fn send(&mut self, data: &[u8]) -> isize {
            take("send", data.len().cast_signed())
        }
        fn recv(&mut self, _: &mut [u8]) -> isize {
            take("recv", 2)
        }
        fn socks5_negotiate(&mut self) -> Vec<u8> {
            let length = take("negotiate", 3);
            vec![5; usize::try_from(length).unwrap_or(0)]
        }
        fn socks5_step(&mut self, _: &[u8]) -> Option<bool> {
            match take("step", 1) {
                1 => Some(true),
                0 => Some(false),
                _ => None,
            }
        }
        fn http_request(&mut self) -> Vec<u8> {
            take("request", 7);
            b"CONNECT".to_vec()
        }
        fn http_response(&mut self) -> (isize, bool) {
            let accepted = take("accepted", 1) != 0;
            (take("response", 10), accepted)
        }
        fn http_done(&mut self, received: bool) {
            log(format!("done {received}"));
        }
        fn client(&mut self) {
            log("client".to_owned());
        }
        fn handshake(&mut self) -> bool {
            take("handshake", 1) != 0
        }
    }

    fn connect_ops(proxy: Option<Proxy>, ssl: bool, family: c_int) -> Mock {
        Mock {
            proxy,
            ssl,
            family,
            http_handshake: true,
        }
    }

    fn run(ops: &mut Mock, host: &str, script: &[(&'static str, isize)]) -> (bool, Vec<String>) {
        LOG.with(|log| log.borrow_mut().clear());
        SCRIPT.with(|s| *s.borrow_mut() = script.to_vec());
        let ok = connect_host(ops, host.as_bytes(), 80);
        (ok, LOG.with(|log| log.borrow().clone()))
    }

    /// resolve.rs's `assign_resolve_and_retry`, over the trait: the scripted assigns
    /// and lookups, and whether the last error is a bad host address.
    fn resolved(
        assigns: &[bool],
        lookups: &[bool],
        bad: bool,
    ) -> (Result<(), ResolveError>, Vec<String>) {
        let mut script: Vec<(&'static str, isize)> = Vec::new();
        script.extend(assigns.iter().map(|&a| ("assign", isize::from(a))));
        script.extend(lookups.iter().map(|&l| ("lookup", isize::from(l))));
        script.extend(core::iter::repeat_n(("bad_host", isize::from(bad)), 2));
        LOG.with(|log| log.borrow_mut().clear());
        SCRIPT.with(|s| *s.borrow_mut() = script);
        let code = resolve(&mut connect_ops(None, false, 0));
        let log = LOG.with(|log| log.borrow().clone());
        (code, log.into_iter().filter(|e| e != "bad_host").collect())
    }

    #[test]
    fn assign_resolve_and_retry() {
        let names = |names: &[&str]| names.iter().map(|&n| n.to_owned()).collect::<Vec<_>>();
        assert_eq!(
            resolved(&[true], &[], false),
            (Ok(()), names(&["assign"])),
            "an address"
        );
        assert_eq!(
            resolved(&[false, true], &[true], true),
            (Ok(()), names(&["assign", "lookup", "assign"])),
            "a name"
        );
        assert_eq!(
            resolved(&[false], &[], false),
            (Err(ResolveError::Assign), names(&["assign"])),
            "another error"
        );
        assert_eq!(
            resolved(&[false], &[false], true),
            (Err(ResolveError::Lookup), names(&["assign", "lookup"]))
        );
        // A second bad host address resolves again and falls through, as upstream's loop does.
        assert_eq!(
            resolved(&[false, false], &[true, true], true),
            (Ok(()), names(&["assign", "lookup", "assign", "lookup"]))
        );
    }

    #[test]
    fn stoi_reads_a_leading_number() {
        assert_eq!(stoi(b"8080"), 8080);
        assert_eq!(stoi(b" -12x"), -12);
        assert_eq!(stoi(b"x"), 0);
    }

    #[test]
    fn direct_connect() {
        let mut ops = connect_ops(None, false, libc::AF_INET);
        assert_eq!(
            run(&mut ops, "example.com", &[]),
            (
                true,
                vec![
                    "available",
                    "host Connect example.com 80",
                    "assign",
                    "connect",
                    "client"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            )
        );
        let (ok, log) = run(&mut ops, "x", &[("available", 0)]);
        assert!(!ok);
        assert_eq!(log, ["available"]);
    }

    #[test]
    fn resolve_failure_reports_the_last_error_text() {
        let mut ops = connect_ops(None, false, libc::AF_INET);
        let (ok, log) = run(
            &mut ops,
            "x",
            &[("assign", 0), ("bad_host", 0), ("last_error", 704)],
        );
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err 704 strerror");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_pton")]
    fn tls_host_name_and_proxy_family_switch() {
        // An IPv4 socket whose proxy (::1) is IPv6 reopens as TCP6.
        let mut ops = connect_ops(Some(Proxy::Socks5), true, libc::AF_INET);
        let (ok, log) = run(&mut ops, "example.com", &[]);
        assert!(ok);
        assert_eq!(
            log[..5],
            [
                "available",
                "host Tls example.com 0",
                "host Proxy example.com 80",
                "reinit6",
                "assign"
            ]
        );
        assert_eq!(
            log[5..],
            [
                "connect",
                "negotiate",
                "send",
                "recv",
                "step",
                "client",
                "handshake"
            ]
        );
        // An IP host is no TLS host name; a failed reopen stops.
        let (ok, log) = run(&mut ops, "10.0.0.1", &[("reinit6", 0)]);
        assert!(!ok);
        assert_eq!(log, ["available", "host Proxy 10.0.0.1 80", "reinit6"]);
    }

    #[test]
    fn socks5_failures() {
        let mut ops = connect_ops(Some(Proxy::Socks5), false, 0);
        let (ok, log) = run(&mut ops, "h", &[("recv", 2), ("step", 0), ("recv", 0)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err 7005");
        assert_eq!(log.iter().filter(|e| *e == "recv").count(), 2);
        let (ok, log) = run(&mut ops, "h", &[("send", -1)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 2..], ["send", "err 7005"]);
    }

    #[test]
    fn http_proxy() {
        let mut ops = connect_ops(Some(Proxy::Http), false, 0);
        let (ok, log) = run(&mut ops, "h", &[]);
        assert!(ok);
        assert!(log.contains(&"done true".to_owned()));
        let (ok, log) = run(&mut ops, "h", &[("accepted", 0)]);
        assert!(!ok);
        assert_eq!(
            log[log.len() - 3..],
            [
                "err 7104 wrong http_proxy response received, \n[Request]: CONNECT\n[Response]: CONNECT",
                "done true",
                "err 7103"
            ]
        );
        let (ok, log) = run(&mut ops, "h", &[("send", 3)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 2..], ["done false", "err 7103"]);
        let mut quiet = connect_ops(Some(Proxy::Http), false, 0);
        quiet.http_handshake = false;
        let (ok, log) = run(&mut quiet, "h", &[]);
        assert!(ok);
        assert!(!log.contains(&"request".to_owned()));
    }

    #[test]
    fn tls_failure_without_error_sets_handshake_failed() {
        let mut ops = connect_ops(None, true, 0);
        let (ok, log) = run(&mut ops, "h", &[("handshake", 0)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err 1018");
        let (ok, log) = run(&mut ops, "h", &[("handshake", 0), ("last_error", 1014)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "last_error");
    }

    /// A scripted handshake: each effect logs its name and takes its scripted result.
    struct Script {
        mode: HandshakeMode,
        handshaked: bool,
        context_ready: bool,
        verify_peer: bool,
    }

    impl Handshaking for Script {
        fn mode(&self) -> HandshakeMode {
            self.mode
        }
        fn handshaked(&self) -> bool {
            self.handshaked
        }
        fn context_ready(&self) -> bool {
            self.context_ready
        }
        fn verify_peer(&self) -> bool {
            self.verify_peer
        }
        fn available(&mut self) -> bool {
            take("available", 1) != 0
        }
        fn set_err(&mut self, code: c_int) {
            log(format!("err {code}"));
        }
        fn context_create(&mut self) -> bool {
            take("context_create", 1) != 0
        }
        fn create(&mut self) -> bool {
            take("create", 1) != 0
        }
        /// Scripted as the client's SSL state, the server's `swReturnCode` or the
        /// self-driven step's 1 done, 0 again, -1 failed.
        fn step(&mut self) -> Step {
            match (self.mode, take("tls_step", 1)) {
                (HandshakeMode::Client | HandshakeMode::SelfDriven, 1)
                | (HandshakeMode::Server, 5) => Step::Ready,
                (HandshakeMode::Client | HandshakeMode::Server, 2) => Step::Wait,
                (HandshakeMode::Client, 0..) | (HandshakeMode::SelfDriven, 0) => Step::Again,
                _ => Step::Failed,
            }
        }
        fn wait(&mut self, fresh: bool) -> bool {
            take(if fresh { "wait_fresh" } else { "wait" }, 1) != 0
        }
        fn verify(&mut self) -> bool {
            take("verify", 1) != 0
        }
    }

    fn tls(mode: HandshakeMode, script: &[(&'static str, isize)]) -> (bool, Vec<String>) {
        LOG.with(|log| log.borrow_mut().clear());
        SCRIPT.with(|s| *s.borrow_mut() = script.to_vec());
        let mut ops = Script {
            mode,
            handshaked: false,
            context_ready: false,
            verify_peer: true,
        };
        let ok = ssl_handshake(&mut ops);
        (ok, LOG.with(|log| log.borrow().clone()))
    }

    #[test]
    fn client_handshake() {
        let (ok, log) = tls(
            HandshakeMode::Client,
            &[("tls_step", 2), ("tls_step", 0), ("tls_step", 1)],
        );
        assert!(ok);
        assert_eq!(
            log,
            [
                "available",
                "context_create",
                "create",
                "tls_step",
                "wait_fresh",
                "tls_step",
                "tls_step",
                "verify"
            ]
        );
        let (ok, log) = tls(HandshakeMode::Client, &[("tls_step", -1)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err -1");
        let (ok, log) = tls(HandshakeMode::Client, &[("tls_step", 2), ("wait_fresh", 0)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "wait_fresh");
    }

    #[test]
    fn server_handshake() {
        let (ok, log) = tls(HandshakeMode::Server, &[("tls_step", 2), ("tls_step", 5)]);
        assert!(ok);
        assert_eq!(log[3..], ["tls_step", "wait", "tls_step", "verify"]);
        // A failed wait is a bad client too.
        let (ok, log) = tls(HandshakeMode::Server, &[("tls_step", 2), ("wait", 0)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err 1015");
    }

    #[test]
    fn self_driven_handshake_and_preconditions() {
        let (ok, log) = tls(
            HandshakeMode::SelfDriven,
            &[("tls_step", 0), ("tls_step", 1), ("verify", 0)],
        );
        assert!(!ok);
        assert_eq!(log[3..], ["tls_step", "tls_step", "verify"]);
        let (ok, log) = tls(HandshakeMode::SelfDriven, &[("create", 0)]);
        assert!(!ok);
        assert_eq!(log, ["available", "context_create", "create"]);
        LOG.with(|log| log.borrow_mut().clear());
        let mut ops = Script {
            mode: HandshakeMode::Client,
            handshaked: true,
            context_ready: true,
            verify_peer: false,
        };
        assert!(!ssl_handshake(&mut ops));
        assert_eq!(LOG.with(|log| log.borrow().clone()), ["err 509"]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no inet_pton")]
    fn ip_checks_match_inet_pton() {
        assert!(is_ip(libc::AF_INET, b"127.0.0.1"));
        assert!(!is_ip(libc::AF_INET, b"localhost"));
        assert!(is_ip(libc::AF_INET6, b"::1"));
        assert!(!is_ip(libc::AF_INET6, b"127.0.0.1"));
        assert!(
            is_ip(libc::AF_INET, b"1.2.3.4\0junk"),
            "c_str() stops at the NUL"
        );
    }
}
