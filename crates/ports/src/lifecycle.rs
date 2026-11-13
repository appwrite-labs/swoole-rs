//! `ListenPort`'s TLS setup, SNI selection, socket import and close, over the
//! records Rust lays out: the port's settings and TLS state, its `SSLContext`s'
//! Rust owners, the socket record and OpenSSL. Logs name the upstream function
//! that wrote them.
//!
//! `PortConfig::ssl_context` holds one reference to the port's own context's
//! owner, and its TLS state (`PortConfig::tls`) what upstream's `ListenPort` kept
//! in C++ containers: the SNI contexts (`sni_contexts`) and the DTLS sessions
//! (`dtls_sessions`), each with a reference to its context.
use core::ffi::{CStr, c_int, c_void};
use core::ptr;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{MutexGuard, PoisonError, RwLockReadGuard};

use swoole_rs_clients::address::{swr_v1_clients_address_port, with_host};
use swoole_rs_corebase::constants::{SW_SSL_CIPHER_LIST, SW_SSL_ECDH_CURVE};
use swoole_rs_corebase::{log, sys};
use swoole_rs_reactor::{Socket, SocketRecord, swr_v1_socket_type};
use swoole_rs_server::port_config::{DtlsSessions, PortConfig, TlsState};
use swoole_rs_server::view;
use swoole_rs_tls::upstream::socket_ssl_create;
use swoole_rs_tls::{
    DtlsSession, OwnedTlsContext, OwnedTlsFields, Ssl, SslCreateFlag, SslVersion, Text, TlsContext,
    has_files, swr_v1_tls_dtls_free, swr_v1_tls_dtls_init, swr_v1_tls_dtls_new,
    swr_v1_tls_owner_view, swr_v1_tls_port_index,
};
use swoole_rs_utilities::sni_wildcard_matches;

use crate::methods::{FD_DGRAM_SERVER, FD_STREAM_SERVER, socket_free};

/// `SW_LOG_WARNING`, `SW_LOG_ERROR`, `SW_ERROR_WRONG_OPERATION` and
/// `SW_ERROR_SSL_CREATE_SESSION_FAILED`.
const LOG_WARNING: c_int = 4;
const LOG_ERROR: c_int = 5;
const WRONG_OPERATION: c_int = 509;
const SSL_CREATE_SESSION_FAILED: c_int = 1020;
/// `SSL_TLSEXT_ERR_OK` and `SSL_TLSEXT_ERR_NOACK`.
const TLSEXT_OK: c_int = 0;
const TLSEXT_NOACK: c_int = 3;

/// The port's SNI contexts, read.
fn sni(config: &PortConfig) -> RwLockReadGuard<'_, Vec<(Vec<u8>, TlsContext)>> {
    config
        .tls
        .sni
        .read()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The port's DTLS sessions.
fn dtls(config: &PortConfig) -> MutexGuard<'_, DtlsSessions> {
    config
        .tls
        .dtls
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// `edit`'s result over the fields of the live `context`.
fn edit<R>(context: &OwnedTlsContext, edit: impl FnOnce(&mut OwnedTlsFields) -> R) -> R {
    // SAFETY: the owner's fields live as long as the reference; a port's contexts
    // are used on its one thread, and every `edit` closure here ends without
    // reaching another.
    edit(unsafe { &mut *swr_v1_tls_owner_view(context).fields })
}

/// Releases the port's context and its TLS state with the SNI contexts.
pub(crate) fn tls_free(config: &mut PortConfig) {
    config.ssl_context = None;
    *config.tls = TlsState::default();
}

/// `ListenPort::dup_ssl_context()`: a new context with the port's settings.
#[must_use]
pub fn context_dup(config: &PortConfig) -> TlsContext {
    let copy = TlsContext::new();
    // Without a context of the port's, the fresh one's defaults.
    let Some(source) = config.ssl_context.as_ref() else {
        return copy;
    };
    // SAFETY: the copy is fresh; no native view caches either's strings.
    if unsafe { swoole_rs_tls::swr_v1_tls_owner_assign(&copy, source) }.status != 0 {
        swoole_rs_corebase::fatal("copying a port's TLS context failed");
    }
    copy
}

/// `ListenPort::ssl_context_create(context, allow_empty_cert)`: both files or
/// neither (and neither only when allowed), the port's HTTP flags, then the
/// `SSL_CTX`.
pub(crate) fn context_create(
    config: &PortConfig,
    context: &OwnedTlsContext,
    allow_empty_cert: bool,
) -> bool {
    let (cert, key) = has_files(context);
    if cert != key || (!cert && !allow_empty_cert) {
        log::error_log(
            LOG_ERROR,
            WRONG_OPERATION,
            c"ListenPort::ssl_context_create",
            format_args!("require `ssl_cert_file` and `ssl_key_file` options"),
        );
        return false;
    }
    edit(context, |fields| {
        if config.open_http_protocol.get() {
            fields.http = true;
        }
        if config.open_http2_protocol.get() {
            fields.http_v2 = true;
        }
    });
    // SAFETY: the live owner.
    if !unsafe { swoole_rs_tls::ssl_context_create(context) } {
        log::warning(
            c"ListenPort::ssl_context_create",
            format_args!("failed to create ssl content"),
        );
        return false;
    }
    true
}

/// `ListenPort::ssl_add_sni_cert(name, ctx)`: creates the context (both files
/// required) and stores it under `name` unless the name is taken.
pub fn sni_add(config: &PortConfig, name: &[u8], context: TlsContext) -> bool {
    let added = context_create(config, &context, false);
    let mut sni = config
        .tls
        .sni
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    if added && !sni.iter().any(|(stored, _)| stored == name) {
        sni.push((name.to_vec(), context));
    }
    added
}

/// `ListenPort::ssl_init()`: the port's context (no files allowed when SNI
/// contexts exist), and the SNI callback when they do.
#[must_use]
pub fn ssl_init(config: &PortConfig) -> bool {
    let Some(context) = config.ssl_context.clone() else {
        return false;
    };
    let has_sni = !sni(config).is_empty();
    if !context_create(config, &context, has_sni) {
        return false;
    }
    if has_sni {
        // The SSL_CTX `context_create` just made.
        edit(&context, |fields| {
            if let Some(ctx) = fields.context.as_deref() {
                ctx.set_servername_callback(sni_callback);
            }
        });
    }
    true
}

/// `ListenPort::ssl_context_init()`: a new default context with upstream's flags
/// and ciphers; DTLS on a datagram port.
pub(crate) fn ssl_context_init(config: &mut PortConfig) -> bool {
    let context = TlsContext::with_texts(&[
        (Text::Ciphers, SW_SSL_CIPHER_LIST.as_bytes()),
        (Text::EcdhCurve, SW_SSL_ECDH_CURVE.as_bytes()),
    ]);
    let context = config.ssl_context.insert(context).clone();
    edit(&context, |fields| {
        fields.prefer_server_ciphers = true;
        fields.session_tickets = false;
        fields.stapling = true;
        fields.stapling_verify = true;
    });
    // crates/tls requires OpenSSL 3, which has DTLS.
    if matches!(config.r#type, 2 | 4 | 6) {
        let protocols = SslVersion::SW_SSL_DTLS as u32;
        edit(&context, |fields| fields.protocols = protocols);
        dtls(config).clear();
    }
    true
}

/// `ListenPort::ssl_create(sock)`: the socket's server-side `SSL`, which names the port.
pub(crate) fn ssl_create(config: &PortConfig, sock: Option<Socket<'_>>) -> bool {
    let context = config.ssl_context.as_ref();
    let flags = SslCreateFlag::SW_SSL_SERVER as c_int;
    let create = |sock| {
        context.is_some_and(|context| {
            // SAFETY: nothing writes the port's context while a connection's session is
            // made.
            unsafe { socket_ssl_create(sock, context, flags) >= 0 }
        })
    };
    let Some(sock) = sock.filter(|&sock| create(sock)) else {
        log::set_last_error(SSL_CREATE_SESSION_FAILED);
        return false;
    };
    // The port outlives its connections; the SNI callback only reads it.
    let port = ptr::from_ref(config).cast_mut().cast();
    if !Ssl::of(sock).is_some_and(|ssl| ssl.set_ex_data(swr_v1_tls_port_index(), port)) {
        log::warning(
            c"ListenPort::ssl_create",
            format_args!("SSL_set_ex_data() failed"),
        );
        return false;
    }
    true
}

/// OpenSSL's servername callback, as `ListenPort::ssl_server_sni_callback`: the
/// first SNI context whose name matches wins.
extern "C" fn sni_callback(ssl: Ssl<'_>, _al: *mut c_int, _arg: *mut c_void) -> c_int {
    let Some(name) = ssl.servername() else {
        return TLSEXT_NOACK;
    };
    let name = name.as_slice();
    // `ssl_create` stored its port.
    let port = ssl.ex_data(swr_v1_tls_port_index());
    // SAFETY: the port the handshake's connection belongs to, live while it serves:
    // only a port's own contexts hold this callback, and `ssl_create` names the port
    // on each SSL it makes from one. Handshakes on every reactor thread read it at
    // once, through `&` only.
    let Some(port) = (unsafe { view::Port::from_raw(port) }) else {
        return TLSEXT_NOACK;
    };
    let sni = sni(port.config());
    let Some((_, context)) = sni
        .iter()
        .find(|(cert, _)| sni_wildcard_matches(name, cert))
    else {
        return TLSEXT_NOACK;
    };
    // That context's live SSL_CTX; switching to it calls nothing back.
    edit(context, |fields| ssl.set_context(fields.context.as_deref()));
    TLSEXT_OK
}

/// The port's DTLS session on `fd`; null if none.
pub(crate) fn dtls_session(config: &PortConfig, fd: c_int) -> *mut DtlsSession {
    let sessions = dtls(config);
    let entry = usize::try_from(fd).ok().and_then(|fd| sessions.get(fd));
    entry
        .and_then(Option::as_ref)
        .map_or(ptr::null_mut(), |entry| entry.0)
}

/// `delete` of the DTLS session on `fd`, removed from the port; nothing if none.
/// Nothing uses the session afterwards.
pub(crate) fn dtls_free(config: &PortConfig, fd: c_int) {
    // Out of the table first, its lock released.
    let removed = usize::try_from(fd)
        .ok()
        .and_then(|fd| dtls(config).get_mut(fd).and_then(Option::take));
    if let Some((session, _context)) = removed {
        // SAFETY: the session the port owned, released once before its context.
        unsafe { swr_v1_tls_dtls_free(session) };
    }
}

/// `ListenPort::create_dtls_session(sock)`: a session over the port's context,
/// stored under the socket's fd; null when it fails to initialize.
pub(crate) fn dtls_create(port: view::Port<'_>, sock: Socket<'_>) -> *mut DtlsSession {
    let config = port.config();
    let Some(context) = config.ssl_context.clone() else {
        return ptr::null_mut();
    };
    let Ok(fd) = usize::try_from(sock.fd()) else {
        return ptr::null_mut();
    };
    let session = swr_v1_tls_dtls_new(sock.as_ptr());
    // SAFETY: the fresh session over the caller's live socket, and the port's live context.
    if !unsafe { swr_v1_tls_dtls_init(session, &context) } {
        // SAFETY: the session made above, which nothing else holds.
        unsafe { swr_v1_tls_dtls_free(session) };
        return ptr::null_mut();
    }
    let mut sessions = dtls(config);
    if sessions.len() <= fd {
        sessions.resize_with(fd + 1, || None);
    }
    // As the C++ map's assignment, the fd's earlier entry goes.
    if let Some(slot) = sessions.get_mut(fd) {
        *slot = Some((session, context));
    }
    drop(sessions);
    session
}

/// `getsockopt(fd, SOL_SOCKET, option)`: the value, or `None` (errno set).
fn get_option(fd: c_int, option: c_int) -> Option<c_int> {
    let mut value: c_int = 0;
    let got = swoole_rs_corebase::sys::getsockopt_int(fd, libc::SOL_SOCKET, option, &mut value);
    (got == 0).then_some(value)
}

/// `ListenPort::import(fd)`: adopts a listening socket; the fd stays the
/// caller's when it isn't one. Upstream's port keeps pointing at the freed
/// record on failure; this one never takes it.
pub(crate) fn import(config: &mut PortConfig, fd: c_int) -> bool {
    let origin = c"ListenPort::import";
    let mut socket = SocketRecord::with_fd(fd, 0);
    let Some(so_type) = get_option(fd, libc::SO_TYPE) else {
        let message = format_args!("getsockopt({fd}, SOL_SOCKET, SO_TYPE) failed");
        log::sys_warning(origin, message);
        return false;
    };
    if socket.get_name() < 0 {
        log::sys_warning(origin, format_args!("getsockname({fd}) failed"));
        return false;
    }
    let Some(acceptconn) = get_option(fd, libc::SO_ACCEPTCONN) else {
        let message = format_args!("getsockopt({fd}, SOL_SOCKET, SO_ACCEPTCONN) failed");
        log::sys_warning(origin, message);
        return false;
    };
    if acceptconn == 0 {
        let message = format_args!("the socket[{fd}] is not a listening socket");
        log::error_log(LOG_WARNING, libc::EINVAL, origin, message);
        return false;
    }
    let family = c_int::from(socket.info.addr.family());
    let kind = swr_v1_socket_type(family, so_type);
    socket.socket_type = kind;
    socket.info.kind = kind;
    socket.fd_type = if matches!(kind, 2 | 4 | 6) {
        FD_DGRAM_SERVER
    } else {
        FD_STREAM_SERVER
    };
    socket.removed = true;
    let info = socket.info;
    *config.socket.get_mut() = socket.into_raw().cast();
    config.r#type = kind;
    if with_host(&info, |host| config.host.set(host.to_bytes())) != 0 {
        swoole_rs_corebase::fatal("out of memory storing a port's host");
    }
    config.port.set(swr_v1_clients_address_port(&info));
    config.listening.set(true);
    true
}

/// `ListenPort::close()`: drops the TLS context and DTLS map, frees the socket
/// and unlinks a Unix socket's path.
pub(crate) fn close(port: view::Port<'_>) {
    if port.config().ssl != 0 {
        // SAFETY: the server closes its ports as it shuts down, once nothing else
        // reads their settings (`PortMethods::close`), and no borrow of them is
        // held here: only the context is replaced.
        unsafe { port.config_mut() }.ssl_context = None;
        dtls(port.config()).clear();
    }
    let config = port.config();
    // Taken first, so nothing finds the record once it is freed.
    let socket = config.socket.swap(ptr::null_mut(), Relaxed);
    if !socket.is_null() {
        // SAFETY: the port's live socket, which nothing uses after.
        unsafe { socket_free(socket.cast()) };
    }
    if matches!(config.r#type, 5 | 6) {
        let mut path = config.host_bytes().to_vec();
        path.push(0);
        if let Ok(path) = CStr::from_bytes_until_nul(&path) {
            sys::unlink(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use swoole_rs_utilities::sni_wildcard_matches;

    #[test]
    fn the_tls_state_lives_in_the_port_until_freed() {
        let config = swoole_rs_server::port_config::swr_v1_port_config_new(0);
        let config = unsafe { &mut *config };
        assert_eq!(super::sni(config).len(), 0);
        config
            .tls
            .sni
            .write()
            .unwrap()
            .push((b"a".to_vec(), swoole_rs_tls::TlsContext::new()));
        super::dtls(config).resize_with(4, || None);
        super::dtls(config)[3] = Some((core::ptr::null_mut(), swoole_rs_tls::TlsContext::new()));
        assert_eq!(super::sni(config).len(), 1);
        assert_eq!(super::dtls(config).iter().flatten().count(), 1);
        super::tls_free(config);
        assert_eq!(super::sni(config).len(), 0);
        assert_eq!(super::dtls(config).iter().flatten().count(), 0);
        drop(unsafe { Box::from_raw(config) });
    }

    /// `TEST(server, ssl_matches_wildcard_name)`: upstream's 44 live `EXPECT`s, in its
    /// order (its commented-out ones stay out), each as `(subject, cert, expected)`.
    #[test]
    fn server_ssl_matches_wildcard_name() {
        let cases = [
            // Test exact match
            ("example.com", "example.com", true),
            ("test.example.com", "test.example.com", true),
            ("EXAMPLE.COM", "example.com", true),
            ("example.com", "EXAMPLE.COM", true),
            // Test no match
            ("example.com", "example.org", false),
            ("test.example.com", "test.example.org", false),
            ("sub.example.com", "example.com", false),
            ("example.com", "sub.example.com", false),
            // Test wildcard in leftmost component
            ("test.example.com", "*.example.com", true),
            ("sub.example.com", "*.example.com", true),
            ("TEST.example.com", "*.example.com", true),
            ("test.example.com", "*.EXAMPLE.COM", true),
            // Test wildcard with prefix
            ("subtest.example.com", "sub*.example.com", true),
            ("subthing.example.com", "sub*.example.com", true),
            ("wrongtest.example.com", "sub*.example.com", false),
            ("test.example.com", "sub*.example.com", false),
            // Test wildcard in non-leftmost component (should fail)
            ("test.example.com", "test.*.com", false),
            ("test.sub.example.com", "test.*.example.com", false),
            ("example.com", "example.*", false),
            // Test wildcard with dot in prefix (should fail)
            ("test.example.com", "test.*.com", false),
            ("sub.test.example.com", "sub.*.example.com", false),
            // Test multiple wildcards (only first one should be considered)
            ("test.sub.example.com", "*.*example.com", false),
            ("test.example.com", "*.*", false),
            // Test wildcard matching with dots between prefix and suffix
            ("test.sub.example.com", "*.example.com", false),
            ("a.b.c.example.com", "*.example.com", false),
            ("testexample.com", "*example.com", true),
            // Test suffix length conditions
            ("test.com", "*.example.com", false),
            ("short", "*.verylongdomain.com", false),
            ("example.com", "*.example.com", false),
            // Test empty strings and edge cases
            ("example.com", "", false),
            ("", "example.com", false),
            ("*", "*", true),
            ("test", "*est", true),
            // Test wildcard at beginning with no prefix
            ("test.example.com", "*.example.com", true),
            ("example.com", "*example.com", true),
            // Test wildcard at end with no suffix
            ("example.com", "example*", false),
            ("example.com", "example.*", false),
            // Test practical examples from real-world scenarios
            ("www.example.com", "*.example.com", true),
            ("api.example.com", "*.example.com", true),
            ("login.example.com", "*.example.com", true),
            ("sub.api.example.com", "*.example.com", false),
            ("sub.api.example.com", "*.api.example.com", true),
            ("dev-server.example.com", "dev-*.example.com", true),
            ("staging-server.example.com", "*-server.example.com", true),
        ];
        let failed: Vec<_> = cases
            .iter()
            .filter(|&&(subject, cert, expected)| {
                sni_wildcard_matches(subject.as_bytes(), cert.as_bytes()) != expected
            })
            .collect();
        assert!(failed.is_empty(), "{failed:?}");
    }
}
