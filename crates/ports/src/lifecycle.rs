//! `ListenPort`'s TLS setup, SNI selection, socket import and close as whole
//! operations. Each step asks one `call(context, op, arg)` per fact or effect
//! ([`crate::lifecycle_native`] answers them; tests answer with fakes).
use crate::read::PortsCall;
use core::ffi::{CStr, c_char, c_void};
use swoole_rs_utilities::sni_wildcard_matches;

/// Which operation runs.
pub const RUN_SNI_ADD: u32 = 0;
pub const RUN_SSL_INIT: u32 = 1;
pub const RUN_SSL_CREATE: u32 = 2;
pub const RUN_CONTEXT_INIT: u32 = 3;
pub const RUN_SNI: u32 = 4;
pub const RUN_IMPORT: u32 = 5;
pub const RUN_CLOSE: u32 = 6;
pub const RUN_CONTEXT_CREATE: u32 = 7;

// The context being created (`ssl_context_create`): the SNI one or the port's.
pub const OP_CERT_EMPTY: u32 = 0;
pub const OP_KEY_EMPTY: u32 = 1;
pub const OP_LOG_REQUIRE_CERT: u32 = 2;
pub const OP_OPEN_HTTP: u32 = 3;
pub const OP_OPEN_HTTP2: u32 = 4;
pub const OP_SET_HTTP: u32 = 5;
pub const OP_SET_HTTP2: u32 = 6;
/// `context->create()`.
pub const OP_CREATE: u32 = 7;
pub const OP_WARN_CREATE: u32 = 8;
/// `sni_contexts.emplace(name, ctx)`.
pub const OP_SNI_EMPLACE: u32 = 9;
pub const OP_HAS_SNI: u32 = 10;
pub const OP_SET_SNI_CALLBACK: u32 = 11;
/// `sock->ssl_create(ssl_context, SW_SSL_SERVER)`, an `int`.
pub const OP_SOCKET_SSL: u32 = 12;
pub const OP_SET_CREATE_ERROR: u32 = 13;
/// `SSL_set_ex_data(sock->ssl, port index, this)`.
pub const OP_SET_EX_DATA: u32 = 14;
pub const OP_WARN_EX_DATA: u32 = 15;
/// `ssl_context_create`'s `allow_empty_cert`.
pub const OP_ALLOW_EMPTY_CERT: u32 = 16;
// `ssl_context_init`.
/// A new default `SSLContext` becomes the port's.
pub const OP_CONTEXT_NEW: u32 = 20;
/// Sets a flag of it: arg is `CONTEXT_*` and the value, `field << 1 | value`.
pub const OP_CONTEXT_FLAG: u32 = 21;
/// Sets the default cipher list and ECDH curve.
pub const OP_CONTEXT_CIPHERS: u32 = 22;
pub const OP_IS_DGRAM: u32 = 23;
/// `SW_SUPPORT_DTLS`.
pub const OP_DTLS_SUPPORTED: u32 = 24;
/// DTLS protocols and a new session map.
pub const OP_CONTEXT_DTLS: u32 = 25;
pub const OP_WARN_NO_DTLS: u32 = 26;
// The SNI callback.
/// `SSL_get_servername`, a C string or null.
pub const OP_SERVER_NAME: u32 = 30;
/// The port stored in the `SSL` becomes current.
pub const OP_SSL_PORT: u32 = 31;
pub const OP_SNI_EMPTY: u32 = 32;
/// Moves to the first, then the next, SNI entry; returns its name or null at the end.
pub const OP_SNI_NEXT: u32 = 33;
/// `SSL_set_SSL_CTX` to the current entry's context.
pub const OP_SNI_SELECT: u32 = 34;
// `import(fd)`.
/// A new socket over the fd becomes the port's.
pub const OP_SOCKET_NEW: u32 = 40;
/// `getsockopt(SO_TYPE)`, `getsockname`, `getsockopt(SO_ACCEPTCONN)`: `int`s.
pub const OP_GET_TYPE: u32 = 41;
pub const OP_GET_NAME: u32 = 42;
pub const OP_GET_ACCEPTCONN: u32 = 43;
/// The `SO_ACCEPTCONN` value read.
pub const OP_ACCEPTCONN: u32 = 44;
/// Warns with arg `IMPORT_*`.
pub const OP_WARN_IMPORT: u32 = 45;
/// Releases the fd and deletes the socket.
pub const OP_SOCKET_FAIL: u32 = 46;
/// Takes the socket's type, host and port, listening and server fd type.
pub const OP_ADOPT: u32 = 47;
// `close()`.
pub const OP_SSL_ON: u32 = 50;
pub const OP_HAS_CONTEXT: u32 = 51;
pub const OP_RESET_CONTEXT: u32 = 52;
pub const OP_DELETE_DTLS: u32 = 53;
pub const OP_HAS_SOCKET: u32 = 54;
pub const OP_FREE_SOCKET: u32 = 55;
pub const OP_IS_UNIX: u32 = 56;
pub const OP_UNLINK: u32 = 57;

/// `SSLContext` flags `ssl_context_init` sets.
pub const CONTEXT_PREFER_SERVER_CIPHERS: u64 = 0;
pub const CONTEXT_SESSION_TICKETS: u64 = 1;
pub const CONTEXT_STAPLING: u64 = 2;
pub const CONTEXT_STAPLING_VERIFY: u64 = 3;
/// `import` warnings.
pub const IMPORT_TYPE: u64 = 0;
pub const IMPORT_NAME: u64 = 1;
pub const IMPORT_ACCEPTCONN: u64 = 2;
pub const IMPORT_NOT_LISTENING: u64 = 3;

/// `SSL_TLSEXT_ERR_OK` and `SSL_TLSEXT_ERR_NOACK`; native asserts both.
const TLSEXT_OK: i32 = 0;
const TLSEXT_NOACK: i32 = 3;

struct Native {
    context: *mut c_void,
    call: PortsCall,
    config: *const swoole_rs_server::port_config::PortConfig,
}

impl Native {
    fn op(&self, op: u32, arg: u64) -> u64 {
        if !self.config.is_null() {
            // SAFETY: run's caller lends the live port record for this leaf access.
            let config = unsafe { &*self.config };
            match op {
                OP_OPEN_HTTP => return u64::from(config.open_http_protocol),
                OP_OPEN_HTTP2 => return u64::from(config.open_http2_protocol),
                OP_IS_DGRAM => return u64::from(matches!(config.r#type, 2 | 4 | 6)),
                OP_SSL_ON => return u64::from(config.ssl),
                OP_HAS_SOCKET => return u64::from(!config.socket.is_null()),
                OP_IS_UNIX => return u64::from(matches!(config.r#type, 5 | 6)),
                _ => {}
            }
        }
        // SAFETY: a native fact or effect on the live port in `context`.
        unsafe { (self.call)(self.context, op, arg) }
    }
    fn is(&self, op: u32) -> bool {
        self.op(op, 0) != 0
    }
    fn effect(&self, op: u32) {
        self.op(op, 0);
    }
    fn int(&self, op: u32) -> i64 {
        self.op(op, 0).cast_signed()
    }
}

/// The `ListenPort` operation `kind`: a `bool` as 0/1, or the SNI callback's
/// `SSL_TLSEXT_ERR_*`. `call` answers each fact and effect
/// ([`crate::lifecycle_native`]); `config`, when not null, answers the settings ones.
///
/// # Safety
/// `call` acts on `context`, and `config` is null or the port's record, both live
/// for the call; no borrow of the record spans a `call`.
pub(crate) unsafe fn run(
    kind: u32,
    context: *mut c_void,
    call: PortsCall,
    config: *const swoole_rs_server::port_config::PortConfig,
) -> i32 {
    let n = Native {
        context,
        call,
        config,
    };
    match kind {
        RUN_SNI_ADD => {
            let added = context_create(&n, false);
            if added {
                n.effect(OP_SNI_EMPLACE);
            }
            i32::from(added)
        }
        RUN_SSL_INIT => i32::from(ssl_init(&n)),
        RUN_SSL_CREATE => i32::from(ssl_create(&n)),
        RUN_CONTEXT_INIT => i32::from(context_init(&n)),
        RUN_SNI => sni(&n),
        RUN_IMPORT => i32::from(import(&n)),
        RUN_CONTEXT_CREATE => {
            let allow = n.is(OP_ALLOW_EMPTY_CERT);
            i32::from(context_create(&n, allow))
        }
        RUN_CLOSE => {
            close(&n);
            0
        }
        _ => -1,
    }
}

/// `ssl_context_create`: both files or neither (and neither only when allowed).
fn context_create(n: &Native, allow_empty_cert: bool) -> bool {
    let no_cert = n.is(OP_CERT_EMPTY);
    if no_cert != n.is(OP_KEY_EMPTY) || (no_cert && !allow_empty_cert) {
        n.effect(OP_LOG_REQUIRE_CERT);
        return false;
    }
    if n.is(OP_OPEN_HTTP) {
        n.effect(OP_SET_HTTP);
    }
    if n.is(OP_OPEN_HTTP2) {
        n.effect(OP_SET_HTTP2);
    }
    if !n.is(OP_CREATE) {
        n.effect(OP_WARN_CREATE);
        return false;
    }
    true
}

fn ssl_init(n: &Native) -> bool {
    let sni = n.is(OP_HAS_SNI);
    if !context_create(n, sni) {
        return false;
    }
    if !n.is(OP_SNI_EMPTY) {
        n.effect(OP_SET_SNI_CALLBACK);
    }
    true
}

fn ssl_create(n: &Native) -> bool {
    if n.int(OP_SOCKET_SSL) < 0 {
        n.effect(OP_SET_CREATE_ERROR);
        return false;
    }
    if n.int(OP_SET_EX_DATA) == 0 {
        n.effect(OP_WARN_EX_DATA);
        return false;
    }
    true
}

fn context_init(n: &Native) -> bool {
    n.effect(OP_CONTEXT_NEW);
    for (field, value) in [
        (CONTEXT_PREFER_SERVER_CIPHERS, 1),
        (CONTEXT_SESSION_TICKETS, 0),
        (CONTEXT_STAPLING, 1),
        (CONTEXT_STAPLING_VERIFY, 1),
    ] {
        n.op(OP_CONTEXT_FLAG, field << 1 | value);
    }
    n.effect(OP_CONTEXT_CIPHERS);
    if n.is(OP_IS_DGRAM) {
        if !n.is(OP_DTLS_SUPPORTED) {
            n.effect(OP_WARN_NO_DTLS);
            return false;
        }
        n.effect(OP_CONTEXT_DTLS);
    }
    true
}

/// `ssl_server_sni_callback`: the first entry whose name matches wins.
fn sni(n: &Native) -> i32 {
    let name = n.op(OP_SERVER_NAME, 0);
    if name == 0 {
        return TLSEXT_NOACK;
    }
    n.effect(OP_SSL_PORT);
    if n.is(OP_SNI_EMPTY) {
        return TLSEXT_NOACK;
    }
    loop {
        let cert = n.op(OP_SNI_NEXT, 0);
        if cert == 0 {
            return TLSEXT_NOACK;
        }
        // SAFETY: both are NUL-terminated strings native code keeps alive for the call.
        if unsafe { ssl_matches_wildcard_name(name as *const c_char, cert as *const c_char) } {
            n.effect(OP_SNI_SELECT);
            return TLSEXT_OK;
        }
    }
}

fn import(n: &Native) -> bool {
    n.effect(OP_SOCKET_NEW);
    for (query, warning) in [
        (OP_GET_TYPE, IMPORT_TYPE),
        (OP_GET_NAME, IMPORT_NAME),
        (OP_GET_ACCEPTCONN, IMPORT_ACCEPTCONN),
    ] {
        if n.int(query) < 0 {
            n.op(OP_WARN_IMPORT, warning);
            n.effect(OP_SOCKET_FAIL);
            return false;
        }
    }
    if n.int(OP_ACCEPTCONN) == 0 {
        n.op(OP_WARN_IMPORT, IMPORT_NOT_LISTENING);
        n.effect(OP_SOCKET_FAIL);
        return false;
    }
    n.effect(OP_ADOPT);
    true
}

fn close(n: &Native) {
    if n.is(OP_SSL_ON) {
        if n.is(OP_HAS_CONTEXT) {
            n.effect(OP_RESET_CONTEXT);
        }
        n.effect(OP_DELETE_DTLS);
    }
    if n.is(OP_HAS_SOCKET) {
        n.effect(OP_FREE_SOCKET);
    }
    if n.is(OP_IS_UNIX) {
        n.effect(OP_UNLINK);
    }
}

/// `ListenPort::ssl_matches_wildcard_name`.
///
/// # Safety
/// Both are NUL-terminated strings, live for the call.
#[must_use]
pub unsafe fn ssl_matches_wildcard_name(subject: *const c_char, cert: *const c_char) -> bool {
    // SAFETY: a live C string, by the caller's contract.
    let subject = unsafe { CStr::from_ptr(subject) };
    // SAFETY: a live C string, by the caller's contract.
    let cert = unsafe { CStr::from_ptr(cert) };
    sni_wildcard_matches(subject.to_bytes(), cert.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Port {
        facts: Vec<(u32, u64)>,
        effects: Vec<(u32, u64)>,
        names: Vec<std::ffi::CString>,
        next: usize,
    }

    unsafe extern "C" fn call(context: *mut c_void, op: u32, arg: u64) -> u64 {
        let port = unsafe { &mut *context.cast::<Port>() };
        if op == OP_SNI_NEXT {
            let name = port.names.get(port.next).map_or(0, |n| n.as_ptr() as u64);
            port.next += 1;
            return name;
        }
        if let Some(&(_, value)) = port.facts.iter().find(|(fact, _)| *fact == op) {
            return value;
        }
        let queries = [
            OP_CERT_EMPTY,
            OP_KEY_EMPTY,
            OP_OPEN_HTTP,
            OP_OPEN_HTTP2,
            OP_CREATE,
            OP_HAS_SNI,
            OP_SNI_EMPTY,
            OP_SOCKET_SSL,
            OP_SET_EX_DATA,
            OP_IS_DGRAM,
            OP_DTLS_SUPPORTED,
            OP_SSL_ON,
            OP_HAS_CONTEXT,
            OP_HAS_SOCKET,
            OP_IS_UNIX,
        ];
        if !queries.contains(&op)
            && ![OP_GET_TYPE, OP_GET_NAME, OP_GET_ACCEPTCONN, OP_ACCEPTCONN].contains(&op)
        {
            port.effects.push((op, arg));
        }
        0
    }

    fn run(kind: u32, port: &mut Port) -> i32 {
        unsafe { super::run(kind, (&raw mut *port).cast(), call, core::ptr::null()) }
    }

    #[test]
    fn context_create_requires_both_files() {
        let mut port = Port {
            facts: vec![(OP_KEY_EMPTY, 1)],
            ..Port::default()
        };
        assert_eq!(run(RUN_SNI_ADD, &mut port), 0, "cert without key");
        assert_eq!(port.effects, [(OP_LOG_REQUIRE_CERT, 0)], "logged");
        let mut port = Port {
            facts: vec![(OP_CREATE, 1), (OP_OPEN_HTTP2, 1)],
            ..Port::default()
        };
        assert_eq!(run(RUN_SNI_ADD, &mut port), 1, "both files");
        assert_eq!(
            port.effects,
            [(OP_SET_HTTP2, 0), (OP_SNI_EMPLACE, 0)],
            "http2 flag, then stored"
        );
        let mut port = Port {
            facts: vec![
                (OP_CERT_EMPTY, 1),
                (OP_KEY_EMPTY, 1),
                (OP_HAS_SNI, 1),
                (OP_CREATE, 1),
            ],
            ..Port::default()
        };
        assert_eq!(run(RUN_SSL_INIT, &mut port), 1, "no files allowed with SNI");
        assert_eq!(
            port.effects,
            [(OP_SET_SNI_CALLBACK, 0)],
            "callback installed"
        );
    }

    #[test]
    fn sni_selects_the_first_match() {
        let name = std::ffi::CString::new("a.example.com").unwrap();
        let mut port = Port {
            facts: vec![(OP_SERVER_NAME, name.as_ptr() as u64)],
            ..Port::default()
        };
        port.names = ["b.example.com", "*.example.com", "a.example.com"]
            .map(|n| std::ffi::CString::new(n).unwrap())
            .into();
        assert_eq!(run(RUN_SNI, &mut port), TLSEXT_OK, "matched");
        assert_eq!(
            (port.next, port.effects.last()),
            (2, Some(&(OP_SNI_SELECT, 0))),
            "second entry selected"
        );
        let mut port = Port::default();
        assert_eq!(run(RUN_SNI, &mut port), TLSEXT_NOACK, "no server name");
    }

    #[test]
    fn import_close_and_context_init() {
        let mut port = Port {
            facts: vec![(OP_GET_NAME, u64::MAX)],
            ..Port::default()
        };
        assert_eq!(run(RUN_IMPORT, &mut port), 0, "getsockname failed");
        assert_eq!(
            port.effects,
            [
                (OP_SOCKET_NEW, 0),
                (OP_WARN_IMPORT, IMPORT_NAME),
                (OP_SOCKET_FAIL, 0)
            ],
            "warned and released"
        );
        let mut port = Port {
            facts: vec![(OP_ACCEPTCONN, 1)],
            ..Port::default()
        };
        assert_eq!(run(RUN_IMPORT, &mut port), 1, "listening socket");
        assert_eq!(port.effects.last(), Some(&(OP_ADOPT, 0)), "adopted");
        let mut port = Port {
            facts: vec![(OP_SSL_ON, 1), (OP_IS_UNIX, 1)],
            ..Port::default()
        };
        run(RUN_CLOSE, &mut port);
        assert_eq!(
            port.effects,
            [(OP_DELETE_DTLS, 0), (OP_UNLINK, 0)],
            "no context or socket to free"
        );
        let mut port = Port {
            facts: vec![(OP_IS_DGRAM, 1)],
            ..Port::default()
        };
        assert_eq!(run(RUN_CONTEXT_INIT, &mut port), 0, "DTLS unsupported");
        assert_eq!(port.effects.last(), Some(&(OP_WARN_NO_DTLS, 0)), "warned");
        assert_eq!(
            port.effects[1..5],
            [
                (OP_CONTEXT_FLAG, 1),
                (OP_CONTEXT_FLAG, 2),
                (OP_CONTEXT_FLAG, 5),
                (OP_CONTEXT_FLAG, 7)
            ],
            "flags"
        );
    }
}
