//! The coroutine socket's client sequence, whole: `connect(host)` (TLS host name,
//! proxy target, the socket family switch, the address loop, the connect, the
//! SOCKS5 and HTTP proxy exchanges and the TLS handshake), and `ssl_handshake`'s
//! steps and waits. Native code keeps the socket, its proxies, buffers and TLS
//! objects, and passes their effects in [`ConnectOps`] and [`TlsOps`].
use crate::resolve::{ResolveOps, swr_v1_hook_resolve};
use core::ffi::{c_char, c_int, c_void};

// SAFETY: libc's declaration; the libc crate doesn't bind it.
unsafe extern "C" {
    fn inet_pton(family: c_int, source: *const c_char, destination: *mut c_void) -> c_int;
}

/// Swoole error codes this sequence sets.
const WRONG_OPERATION: c_int = 509;
const SSL_BAD_CLIENT: c_int = 1015;
const SSL_HANDSHAKE_FAILED: c_int = 1018;
const SOCKS5_HANDSHAKE_FAILED: c_int = 7005;
const HTTP_PROXY_HANDSHAKE_FAILED: c_int = 7103;
const HTTP_PROXY_BAD_RESPONSE: c_int = 7104;
/// `SW_SOCK_TCP` and `SW_SOCK_TCP6`.
const SOCK_TCP: c_int = 1;
const SOCK_TCP6: c_int = 3;
/// `ConnectOps::set_host` targets.
const HOST_CONNECT: c_int = 0;
const HOST_TLS: c_int = 1;
const HOST_PROXY: c_int = 2;
/// `ConnectOps::proxy`.
const PROXY_SOCKS5: c_int = 1;
const PROXY_HTTP: c_int = 2;
/// `set_err(-1)` is the argument-less `set_err()`: the last error, else errno.
const LAST_ERROR: c_int = -1;
/// `SW_SSL_STATE_READY`, `SW_SSL_STATE_WAIT_STREAM`; `swReturnCode`'s `SW_WAIT`
/// and `SW_READY`.
const SSL_STATE_READY: c_int = 1;
const SSL_STATE_WAIT_STREAM: c_int = 2;
const WAIT: c_int = 2;
const READY: c_int = 5;

/// The socket's effects for `connect(host)`.
#[repr(C)]
pub struct ConnectOps {
    pub socket: *mut c_void,
    /// `sock_domain`, and `AF_INET`/`AF_INET6` for an IPv4/IPv6 socket, else 0.
    pub domain: c_int,
    pub family: c_int,
    /// 0 without a proxy, 1 SOCKS5, 2 HTTP; whether the HTTP proxy handshakes.
    pub proxy: c_int,
    pub http_handshake: bool,
    /// Whether the socket has an SSL context.
    pub ssl: bool,
    /// `is_available(SW_EVENT_RDWR)`.
    pub available: unsafe extern "C" fn(*mut c_void) -> bool,
    /// `set_err(code)`, or `set_err()` for -1.
    pub set_err: unsafe extern "C" fn(*mut c_void, c_int),
    /// `set_err(code, text)` with `length` bytes of text, or with
    /// `swoole_strerror(code)` when `text` is null.
    pub set_err_text: unsafe extern "C" fn(*mut c_void, c_int, *const c_char, usize),
    /// `swoole_get_last_error()`.
    pub last_error: unsafe extern "C" fn() -> c_int,
    /// Stores `host` (`length` bytes) and `port` as the connect target (0), the
    /// TLS host name (1, no port) or the proxy's target (2), which makes the
    /// proxy the connect target.
    pub set_host: unsafe extern "C" fn(*mut c_void, c_int, *const c_char, usize, c_int),
    /// The connect target's host, its length in `*length`.
    pub connect_host: unsafe extern "C" fn(*mut c_void, *mut usize) -> *const c_char,
    /// `reinit_sock(type)`.
    pub reinit: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    /// The address loop's assign, bad-host check and name lookup (see [`ResolveOps`]).
    pub assign: unsafe extern "C" fn(*mut c_void) -> bool,
    pub bad_host: unsafe extern "C" fn() -> bool,
    pub lookup: unsafe extern "C" fn(*mut c_void) -> bool,
    /// `connect(addr)` of the assigned address.
    pub connect: unsafe extern "C" fn(*mut c_void) -> bool,
    /// The socket's `send` and `recv`.
    pub send: unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> isize,
    pub recv: unsafe extern "C" fn(*mut c_void, *mut c_char, usize) -> isize,
    /// Packs the SOCKS5 negotiation into the proxy's buffer (`*buffer`), moving it
    /// to the handshake state; returns its length.
    pub socks5_negotiate: unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> isize,
    /// `Socks5Proxy::handshake` of a reply (sending what it packs): -1 failed,
    /// 0 continue, 1 ready.
    pub socks5_step: unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> c_int,
    /// Packs the HTTP CONNECT request into the write buffer (`*buffer`); returns its length.
    pub http_request: unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> usize,
    /// Receives the HTTP proxy's response head (`recv_packet` under a "\r\n\r\n"
    /// protocol) and returns its result; on success, whether `HttpProxy::handshake`
    /// accepts it in `*accepted`.
    pub http_response: unsafe extern "C" fn(*mut c_void, *mut bool) -> isize,
    /// Clears the write buffer, and the read buffer when `received`.
    pub http_done: unsafe extern "C" fn(*mut c_void, bool),
    /// `ssl_is_server = false`.
    pub client: unsafe extern "C" fn(*mut c_void),
    /// `ssl_handshake()`.
    pub handshake: unsafe extern "C" fn(*mut c_void) -> bool,
}

/// `Address::verify_ip`: `inet_pton` of the host's `c_str()` for `family` succeeds.
fn is_ip(family: c_int, host: &[u8]) -> bool {
    let text = host.split(|&b| b == 0).next().unwrap_or_default();
    let Ok(text) = std::ffi::CString::new(text) else {
        return false;
    };
    let mut address = [0u8; 16];
    // SAFETY: a NUL-terminated host and 16 writable bytes, enough for either family.
    unsafe { inet_pton(family, text.as_ptr(), address.as_mut_ptr().cast()) == 1 }
}

impl ConnectOps {
    fn set_err(&self, code: c_int) {
        // SAFETY: the live socket's error.
        unsafe { (self.set_err)(self.socket, code) };
    }

    fn set_host(&self, which: c_int, host: &[u8], port: c_int) {
        // SAFETY: the live socket; `host` outlives the call.
        unsafe { (self.set_host)(self.socket, which, host.as_ptr().cast(), host.len(), port) };
    }

    fn connect_host(&self) -> Vec<u8> {
        let mut length = 0;
        // SAFETY: the live socket's connect host and its length.
        let host = unsafe { (self.connect_host)(self.socket, &raw mut length) };
        if host.is_null() {
            return Vec::new();
        }
        // SAFETY: `length` bytes of the socket's string, copied before any other effect.
        unsafe { std::slice::from_raw_parts(host.cast::<u8>(), length) }.to_vec()
    }

    fn resolve(&self) -> c_int {
        let resolve = ResolveOps {
            context: self.socket,
            assign: self.assign,
            bad_host: self.bad_host,
            resolve: self.lookup,
        };
        // SAFETY: a table of the caller's live effects, for this call.
        unsafe { swr_v1_hook_resolve(&raw const resolve) }
    }

    /// `socks5_handshake`: the negotiation, then each reply until the proxy is ready.
    fn socks5(&self) -> bool {
        let mut request = std::ptr::null();
        // SAFETY: the live socket's proxy.
        let length = unsafe { (self.socks5_negotiate)(self.socket, &raw mut request) };
        // Upstream sends the packed length as size_t.
        // SAFETY: the proxy's buffer of `length` bytes.
        if unsafe { (self.send)(self.socket, request, length.cast_unsigned()) } < 0 {
            return false;
        }
        let mut reply = [0u8; 512];
        loop {
            // SAFETY: the live socket into the local buffer.
            let n = unsafe { (self.recv)(self.socket, reply.as_mut_ptr().cast(), reply.len()) };
            if n <= 0 {
                return false;
            }
            // SAFETY: `n` received bytes.
            match unsafe {
                (self.socks5_step)(self.socket, reply.as_ptr().cast(), n.cast_unsigned())
            } {
                1 => return true,
                0 => {}
                _ => return false,
            }
        }
    }

    /// `http_proxy_handshake`: CONNECT, then the response head.
    fn http(&self) -> bool {
        let mut request = std::ptr::null();
        // SAFETY: the live socket's write buffer.
        let length = unsafe { (self.http_request)(self.socket, &raw mut request) };
        // SAFETY: the write buffer's `length` packed bytes.
        if unsafe { (self.send)(self.socket, request, length) } != length.cast_signed() {
            // SAFETY: the live socket's buffers.
            unsafe { (self.http_done)(self.socket, false) };
            return false;
        }
        let mut accepted = false;
        // SAFETY: the live socket.
        let received = unsafe { (self.http_response)(self.socket, &raw mut accepted) };
        let ok = received > 0 && accepted;
        if received > 0 && !accepted {
            // Upstream reports the request as the response too.
            // SAFETY: the write buffer still holds the request.
            let sent = unsafe { std::slice::from_raw_parts(request.cast::<u8>(), length) };
            let mut text = b"wrong http_proxy response received, \n[Request]: ".to_vec();
            text.extend_from_slice(sent);
            text.extend_from_slice(b"\n[Response]: ");
            text.extend_from_slice(sent);
            // SAFETY: the live socket's error and the local text.
            unsafe {
                (self.set_err_text)(
                    self.socket,
                    HTTP_PROXY_BAD_RESPONSE,
                    text.as_ptr().cast(),
                    text.len(),
                );
            }
        }
        // SAFETY: the live socket's buffers.
        unsafe { (self.http_done)(self.socket, true) };
        ok
    }
}

/// `Socket::connect(host, port)`.
///
/// # Safety
/// `ops` is live for the call and `host` denotes `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_connect_host(
    ops: *const ConnectOps,
    host: *const c_char,
    length: usize,
    port: c_int,
) -> bool {
    // SAFETY: the caller passes a live table (see above).
    let ops = unsafe { &*ops };
    // SAFETY: the caller's host bytes, copied before any effect.
    let host = unsafe { std::slice::from_raw_parts(host.cast::<u8>(), length) }.to_vec();
    // SAFETY: the live socket's availability.
    if !unsafe { (ops.available)(ops.socket) } {
        return false;
    }
    let proxied = ops.proxy == PROXY_SOCKS5 || ops.proxy == PROXY_HTTP;
    // A proxy replaces the host, so a TLS host name has to come from it now.
    if ops.ssl && proxied && !is_ip(ops.domain, &host) {
        ops.set_host(HOST_TLS, &host, 0);
    }
    ops.set_host(if proxied { HOST_PROXY } else { HOST_CONNECT }, &host, port);
    if proxied {
        let target = ops.connect_host();
        let switch = if ops.family == libc::AF_INET6 && is_ip(libc::AF_INET, &target) {
            Some(SOCK_TCP)
        } else if ops.family == libc::AF_INET && is_ip(libc::AF_INET6, &target) {
            Some(SOCK_TCP6)
        } else {
            None
        };
        // SAFETY: the live socket.
        if switch.is_some_and(|kind| !unsafe { (ops.reinit)(ops.socket, kind) }) {
            return false;
        }
    }
    if ops.resolve() != 0 {
        // SAFETY: the thread's last Swoole error.
        let error = unsafe { (ops.last_error)() };
        // SAFETY: the live socket's error, with swoole_strerror's text.
        unsafe { (ops.set_err_text)(ops.socket, error, std::ptr::null(), 0) };
        return false;
    }
    // SAFETY: the live socket's connect.
    if !unsafe { (ops.connect)(ops.socket) } {
        return false;
    }
    if ops.proxy == PROXY_SOCKS5 && !ops.socks5() {
        ops.set_err(SOCKS5_HANDSHAKE_FAILED);
        return false;
    }
    if ops.proxy == PROXY_HTTP && ops.http_handshake && !ops.http() {
        ops.set_err(HTTP_PROXY_HANDSHAKE_FAILED);
        return false;
    }
    // SAFETY: the live socket.
    unsafe { (ops.client)(ops.socket) };
    // SAFETY: the live socket's handshake.
    if ops.ssl && !unsafe { (ops.handshake)(ops.socket) } {
        // SAFETY: the thread's last Swoole error.
        if unsafe { (ops.last_error)() } == 0 {
            ops.set_err(SSL_HANDSHAKE_FAILED);
        }
        return false;
    }
    true
}

/// How `TlsOps::step` drives the handshake.
const MODE_CLIENT: c_int = 0;
const MODE_SERVER: c_int = 1;

/// The socket's effects for `ssl_handshake`.
#[repr(C)]
pub struct TlsOps {
    pub socket: *mut c_void,
    /// 0 client (`ssl_connect` steps), 1 server (`ssl_accept` steps), 2 a step
    /// that completes or fails by itself (`io_uring`'s memory BIOs).
    pub mode: c_int,
    pub handshaked: bool,
    /// Whether the SSL context has its `SSL_CTX`, and whether it verifies the peer.
    pub context_ready: bool,
    pub verify_peer: bool,
    /// `is_available(SW_EVENT_RDWR)`.
    pub available: unsafe extern "C" fn(*mut c_void) -> bool,
    /// `set_err(code)`, or `set_err()` for -1.
    pub set_err: unsafe extern "C" fn(*mut c_void, c_int),
    /// `ssl_context_create()`, and `ssl_create` of the socket's context.
    pub context_create: unsafe extern "C" fn(*mut c_void) -> bool,
    pub create: unsafe extern "C" fn(*mut c_void) -> bool,
    /// One step: the client's `ssl_connect()` (-1 when it failed, else the SSL
    /// state), the server's `ssl_accept()` return code, or for mode 2 1 done,
    /// 0 again, -1 failed.
    pub step: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// Waits to read under the read timeout: a timer of its own when `fresh`,
    /// else one timer for the whole handshake.
    pub wait: unsafe extern "C" fn(*mut c_void, bool) -> bool,
    /// `ssl_verify(allow_self_signed)`.
    pub verify: unsafe extern "C" fn(*mut c_void) -> bool,
}

impl TlsOps {
    fn step(&self) -> c_int {
        // SAFETY: the live socket's handshake step.
        unsafe { (self.step)(self.socket) }
    }

    fn wait(&self, fresh: bool) -> bool {
        // SAFETY: the live socket's wait.
        unsafe { (self.wait)(self.socket, fresh) }
    }

    fn set_err(&self, code: c_int) {
        // SAFETY: the live socket's error.
        unsafe { (self.set_err)(self.socket, code) };
    }

    fn steps(&self) -> bool {
        match self.mode {
            MODE_CLIENT => loop {
                match self.step() {
                    code if code < 0 => {
                        self.set_err(LAST_ERROR);
                        return false;
                    }
                    SSL_STATE_WAIT_STREAM => {
                        if !self.wait(true) {
                            return false;
                        }
                    }
                    SSL_STATE_READY => return true,
                    _ => {}
                }
            },
            MODE_SERVER => {
                let mut code = self.step();
                while code == WAIT && self.wait(false) {
                    code = self.step();
                }
                if code != READY {
                    self.set_err(SSL_BAD_CLIENT);
                }
                code == READY
            }
            _ => loop {
                match self.step() {
                    1 => return true,
                    0 => {}
                    _ => return false,
                }
            },
        }
    }
}

/// `Socket::ssl_handshake`, up to marking the socket handshaked, which native
/// code does when this returns true.
///
/// # Safety
/// `ops` is live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_ssl_handshake(ops: *const TlsOps) -> bool {
    // SAFETY: the caller passes a live table (see above).
    let ops = unsafe { &*ops };
    if ops.handshaked {
        ops.set_err(WRONG_OPERATION);
        return false;
    }
    // SAFETY: the live socket's availability.
    let prepared = unsafe { (ops.available)(ops.socket) }
        // SAFETY: the live socket's context.
        && (ops.context_ready || unsafe { (ops.context_create)(ops.socket) })
        // SAFETY: the live socket's SSL.
        && unsafe { (ops.create)(ops.socket) };
    // SAFETY: the live socket's verification.
    prepared && ops.steps() && (!ops.verify_peer || unsafe { (ops.verify)(ops.socket) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

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

    unsafe extern "C" fn available(_: *mut c_void) -> bool {
        take("available", 1) != 0
    }
    unsafe extern "C" fn set_err(_: *mut c_void, code: c_int) {
        log(format!("err {code}"));
    }
    unsafe extern "C" fn set_err_text(_: *mut c_void, code: c_int, text: *const c_char, n: usize) {
        let text = if text.is_null() {
            "strerror".to_owned()
        } else {
            // SAFETY: the test's text.
            String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(text.cast(), n) })
                .into_owned()
        };
        log(format!("err {code} {text}"));
    }
    unsafe extern "C" fn last_error() -> c_int {
        c_int::try_from(take("last_error", 0)).unwrap_or(-9)
    }
    unsafe extern "C" fn set_host(
        _: *mut c_void,
        which: c_int,
        h: *const c_char,
        n: usize,
        port: c_int,
    ) {
        // SAFETY: the caller's host bytes.
        let h = unsafe { std::slice::from_raw_parts(h.cast::<u8>(), n) };
        log(format!(
            "host {which} {} {port}",
            String::from_utf8_lossy(h)
        ));
        if which != HOST_TLS {
            HOST.with(|host| {
                *host.borrow_mut() = if which == HOST_PROXY {
                    b"::1".to_vec()
                } else {
                    h.to_vec()
                };
            });
        }
    }
    unsafe extern "C" fn connect_host(_: *mut c_void, length: *mut usize) -> *const c_char {
        HOST.with(|host| {
            let host = host.borrow();
            // SAFETY: the caller's length slot.
            unsafe { *length = host.len() };
            host.as_ptr().cast()
        })
    }
    unsafe extern "C" fn reinit(_: *mut c_void, kind: c_int) -> bool {
        take(
            if kind == SOCK_TCP6 {
                "reinit6"
            } else {
                "reinit4"
            },
            1,
        ) != 0
    }
    unsafe extern "C" fn assign(_: *mut c_void) -> bool {
        take("assign", 1) != 0
    }
    unsafe extern "C" fn bad_host() -> bool {
        take("bad_host", 0) != 0
    }
    unsafe extern "C" fn lookup(_: *mut c_void) -> bool {
        take("lookup", 1) != 0
    }
    unsafe extern "C" fn connect(_: *mut c_void) -> bool {
        take("connect", 1) != 0
    }
    unsafe extern "C" fn send(_: *mut c_void, _: *const c_char, n: usize) -> isize {
        take("send", n.cast_signed())
    }
    unsafe extern "C" fn recv(_: *mut c_void, _: *mut c_char, _: usize) -> isize {
        take("recv", 2)
    }
    unsafe extern "C" fn socks5_negotiate(_: *mut c_void, buffer: *mut *const c_char) -> isize {
        // SAFETY: the caller's slot.
        unsafe { *buffer = c"\x05\x01".as_ptr() };
        take("negotiate", 3)
    }
    unsafe extern "C" fn socks5_step(_: *mut c_void, _: *const c_char, _: usize) -> c_int {
        c_int::try_from(take("step", 1)).unwrap_or(-9)
    }
    unsafe extern "C" fn http_request(_: *mut c_void, buffer: *mut *const c_char) -> usize {
        // SAFETY: the caller's slot.
        unsafe { *buffer = c"CONNECT".as_ptr() };
        take("request", 7).cast_unsigned()
    }
    unsafe extern "C" fn http_response(_: *mut c_void, accepted: *mut bool) -> isize {
        // SAFETY: the caller's slot.
        unsafe { *accepted = take("accepted", 1) != 0 };
        take("response", 10)
    }
    unsafe extern "C" fn http_done(_: *mut c_void, received: bool) {
        log(format!("done {received}"));
    }
    unsafe extern "C" fn client(_: *mut c_void) {
        log("client".to_owned());
    }
    unsafe extern "C" fn handshake(_: *mut c_void) -> bool {
        take("handshake", 1) != 0
    }

    fn connect_ops(proxy: c_int, ssl: bool, family: c_int) -> ConnectOps {
        ConnectOps {
            socket: std::ptr::null_mut(),
            domain: family,
            family,
            proxy,
            http_handshake: true,
            ssl,
            available,
            set_err,
            set_err_text,
            last_error,
            set_host,
            connect_host,
            reinit,
            assign,
            bad_host,
            lookup,
            connect,
            send,
            recv,
            socks5_negotiate,
            socks5_step,
            http_request,
            http_response,
            http_done,
            client,
            handshake,
        }
    }

    fn run(ops: &ConnectOps, host: &str, script: &[(&'static str, isize)]) -> (bool, Vec<String>) {
        LOG.with(|log| log.borrow_mut().clear());
        SCRIPT.with(|s| *s.borrow_mut() = script.to_vec());
        // SAFETY: a local table and host.
        let ok = unsafe { swr_v1_hook_connect_host(ops, host.as_ptr().cast(), host.len(), 80) };
        (ok, LOG.with(|log| log.borrow().clone()))
    }

    #[test]
    fn direct_connect() {
        let ops = connect_ops(0, false, libc::AF_INET);
        assert_eq!(
            run(&ops, "example.com", &[]),
            (
                true,
                vec![
                    "available",
                    "host 0 example.com 80",
                    "assign",
                    "connect",
                    "client"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            )
        );
        let (ok, log) = run(&ops, "x", &[("available", 0)]);
        assert!(!ok);
        assert_eq!(log, ["available"]);
    }

    #[test]
    fn resolve_failure_reports_the_last_error_text() {
        let ops = connect_ops(0, false, libc::AF_INET);
        let (ok, log) = run(
            &ops,
            "x",
            &[("assign", 0), ("bad_host", 0), ("last_error", 704)],
        );
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err 704 strerror");
    }

    #[test]
    fn tls_host_name_and_proxy_family_switch() {
        // An IPv4 socket whose proxy (::1) is IPv6 reopens as TCP6.
        let ops = connect_ops(PROXY_SOCKS5, true, libc::AF_INET);
        let (ok, log) = run(&ops, "example.com", &[]);
        assert!(ok);
        assert_eq!(
            log[..5],
            [
                "available",
                "host 1 example.com 0",
                "host 2 example.com 80",
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
        let (ok, log) = run(&ops, "10.0.0.1", &[("reinit6", 0)]);
        assert!(!ok);
        assert_eq!(log, ["available", "host 2 10.0.0.1 80", "reinit6"]);
    }

    #[test]
    fn socks5_failures() {
        let ops = connect_ops(PROXY_SOCKS5, false, 0);
        let (ok, log) = run(&ops, "h", &[("recv", 2), ("step", 0), ("recv", 0)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err 7005");
        assert_eq!(log.iter().filter(|e| *e == "recv").count(), 2);
        let (ok, log) = run(&ops, "h", &[("send", -1)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 2..], ["send", "err 7005"]);
    }

    #[test]
    fn http_proxy() {
        let ops = connect_ops(PROXY_HTTP, false, 0);
        let (ok, log) = run(&ops, "h", &[]);
        assert!(ok);
        assert!(log.contains(&"done true".to_owned()));
        let (ok, log) = run(&ops, "h", &[("accepted", 0)]);
        assert!(!ok);
        assert_eq!(
            log[log.len() - 3..],
            [
                "err 7104 wrong http_proxy response received, \n[Request]: CONNECT\n[Response]: CONNECT",
                "done true",
                "err 7103"
            ]
        );
        let (ok, log) = run(&ops, "h", &[("send", 3)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 2..], ["done false", "err 7103"]);
        let mut quiet = connect_ops(PROXY_HTTP, false, 0);
        quiet.http_handshake = false;
        let (ok, log) = run(&quiet, "h", &[]);
        assert!(ok);
        assert!(!log.contains(&"request".to_owned()));
    }

    #[test]
    fn tls_failure_without_error_sets_handshake_failed() {
        let ops = connect_ops(0, true, 0);
        let (ok, log) = run(&ops, "h", &[("handshake", 0)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err 1018");
        let (ok, log) = run(&ops, "h", &[("handshake", 0), ("last_error", 1014)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "last_error");
    }

    unsafe extern "C" fn context_create(_: *mut c_void) -> bool {
        take("context_create", 1) != 0
    }
    unsafe extern "C" fn create(_: *mut c_void) -> bool {
        take("create", 1) != 0
    }
    unsafe extern "C" fn step(_: *mut c_void) -> c_int {
        c_int::try_from(take("tls_step", 1)).unwrap_or(-9)
    }
    unsafe extern "C" fn wait(_: *mut c_void, fresh: bool) -> bool {
        take(if fresh { "wait_fresh" } else { "wait" }, 1) != 0
    }
    unsafe extern "C" fn verify(_: *mut c_void) -> bool {
        take("verify", 1) != 0
    }

    fn tls(mode: c_int, script: &[(&'static str, isize)]) -> (bool, Vec<String>) {
        LOG.with(|log| log.borrow_mut().clear());
        SCRIPT.with(|s| *s.borrow_mut() = script.to_vec());
        let ops = TlsOps {
            socket: std::ptr::null_mut(),
            mode,
            handshaked: false,
            context_ready: false,
            verify_peer: true,
            available,
            set_err,
            context_create,
            create,
            step,
            wait,
            verify,
        };
        // SAFETY: a local table.
        let ok = unsafe { swr_v1_hook_ssl_handshake(&raw const ops) };
        (ok, LOG.with(|log| log.borrow().clone()))
    }

    #[test]
    fn client_handshake() {
        let (ok, log) = tls(
            MODE_CLIENT,
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
        let (ok, log) = tls(MODE_CLIENT, &[("tls_step", -1)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err -1");
        let (ok, log) = tls(MODE_CLIENT, &[("tls_step", 2), ("wait_fresh", 0)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "wait_fresh");
    }

    #[test]
    fn server_handshake() {
        let (ok, log) = tls(MODE_SERVER, &[("tls_step", 2), ("tls_step", 5)]);
        assert!(ok);
        assert_eq!(log[3..], ["tls_step", "wait", "tls_step", "verify"]);
        // A failed wait is a bad client too.
        let (ok, log) = tls(MODE_SERVER, &[("tls_step", 2), ("wait", 0)]);
        assert!(!ok);
        assert_eq!(log[log.len() - 1], "err 1015");
    }

    #[test]
    fn self_driven_handshake_and_preconditions() {
        let (ok, log) = tls(2, &[("tls_step", 0), ("tls_step", 1), ("verify", 0)]);
        assert!(!ok);
        assert_eq!(log[3..], ["tls_step", "tls_step", "verify"]);
        let (ok, log) = tls(2, &[("create", 0)]);
        assert!(!ok);
        assert_eq!(log, ["available", "context_create", "create"]);
        LOG.with(|log| log.borrow_mut().clear());
        let ops = TlsOps {
            socket: std::ptr::null_mut(),
            mode: MODE_CLIENT,
            handshaked: true,
            context_ready: true,
            verify_peer: false,
            available,
            set_err,
            context_create,
            create,
            step,
            wait,
            verify,
        };
        // SAFETY: a local table.
        assert!(!unsafe { swr_v1_hook_ssl_handshake(&raw const ops) });
        assert_eq!(LOG.with(|log| log.borrow().clone()), ["err 509"]);
    }

    #[test]
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
