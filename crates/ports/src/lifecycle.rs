//! `ListenPort`'s TLS setup, SNI selection, socket import and close, over the
//! records Rust lays out: the port's settings and TLS state, its `SSLContext`s'
//! Rust owners, the socket record and OpenSSL. Logs name the upstream function
//! that wrote them.
//!
//! `PortConfig::ssl_context` holds the configuration builder. Its TLS state
//! publishes immutable settings for handshakes and SNI selection. A DTLS session
//! retains its frozen settings through the connection's SSL (crates/tls).
use core::ffi::{CStr, c_int, c_void};
use core::ptr;
use std::sync::{Arc, PoisonError, RwLockReadGuard};

use swoole_rs_clients::address::{address_port, with_host};
use swoole_rs_corebase::constants::{SW_SSL_CIPHER_LIST, SW_SSL_ECDH_CURVE};
use swoole_rs_corebase::utilities::sni_wildcard_matches;
use swoole_rs_corebase::{log, sys};
use swoole_rs_reactor::{OwnedSocket, Socket, SocketRecord, socket_type};
use swoole_rs_server::port_config::{PortConfig, TlsState};
use swoole_rs_server::view;
use swoole_rs_tls::{
    OwnedTlsContext, OwnedTlsFields, Ssl, SslVersion, Text, TlsContext, TlsSettings, has_files,
    swr_v1_tls_port_index,
};

use crate::methods::{FD_DGRAM_SERVER, FD_STREAM_SERVER, socket_free};
use swoole_rs_corebase::log::LogLevel::{SW_LOG_ERROR as LOG_ERROR, SW_LOG_WARNING as LOG_WARNING};

/// `SW_LOG_WARNING`, `SW_LOG_ERROR`, `SW_ERROR_WRONG_OPERATION` and
/// `SW_ERROR_SSL_CREATE_SESSION_FAILED`.
const WRONG_OPERATION: c_int = swoole_rs_error::ErrorCode::SW_ERROR_WRONG_OPERATION as c_int;
const SSL_CREATE_SESSION_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SSL_CREATE_SESSION_FAILED as c_int;
/// `SSL_TLSEXT_ERR_OK` and `SSL_TLSEXT_ERR_NOACK`.
const TLSEXT_OK: c_int = 0;
const TLSEXT_NOACK: c_int = 3;

/// The port's SNI contexts, read.
fn sni(config: &PortConfig) -> RwLockReadGuard<'_, Vec<(Vec<u8>, Arc<TlsSettings>)>> {
    config
        .tls
        .sni
        .read()
        .unwrap_or_else(PoisonError::into_inner)
}

/// `edit`'s result over the fields of the live `context`.
fn edit<R>(context: &OwnedTlsContext, edit: impl FnOnce(&mut OwnedTlsFields) -> R) -> R {
    context.borrow_configuration().edit(edit)
}

/// Releases the port's context and its TLS state with the SNI contexts.
pub(crate) fn tls_free(config: &mut PortConfig) {
    drop(config.ssl_context.replace(None));
    *config.tls = TlsState::default();
}

/// `ListenPort::dup_ssl_context()`: a new context with the port's settings.
#[must_use]
pub fn context_dup(config: &PortConfig) -> TlsContext {
    // Without a context of the port's, the fresh one's defaults.
    let Some(source) = config.ssl_context.get() else {
        return TlsContext::new();
    };
    let mut source = source.borrow_configuration();
    TlsContext::copy_of(&mut source)
        .unwrap_or_else(|| swoole_rs_corebase::fatal("copying a port's TLS context failed"))
}

/// `ListenPort::ssl_context_create(context, allow_empty_cert)`: both files or
/// neither (and neither only when allowed), the port's HTTP flags, then the
/// `SSL_CTX`.
pub(crate) fn context_create(
    config: &PortConfig,
    context: &OwnedTlsContext,
    allow_empty_cert: bool,
    callback: Option<swoole_rs_tls::ServernameCallback>,
) -> Option<TlsSettings> {
    let (cert, key) = has_files(context);
    if cert != key || (!cert && !allow_empty_cert) {
        log::error_log(
            LOG_ERROR,
            WRONG_OPERATION,
            c"ListenPort::ssl_context_create",
            format_args!("require `ssl_cert_file` and `ssl_key_file` options"),
        );
        return None;
    }
    edit(context, |fields| {
        if config.open_http_protocol.get() {
            fields.http = true;
        }
        if config.open_http2_protocol.get() {
            fields.http_v2 = true;
        }
    });
    // The snapshot owns independent fields, strings and OpenSSL configuration.
    let owner = context.borrow_configuration();
    let settings = owner.freeze(callback);
    if settings.is_none() {
        log::warning(
            c"ListenPort::ssl_context_create",
            format_args!("failed to create ssl content"),
        );
    }
    settings
}

/// `ListenPort::ssl_add_sni_cert(name, ctx)`: creates the context (both files
/// required) and stores it under `name` unless the name is taken.
pub fn sni_add(config: &PortConfig, name: &[u8], context: TlsContext) -> bool {
    let Some(settings) = context_create(config, &context, false, None) else {
        return false;
    };
    // The immutable settings own everything the handshake needs.
    drop(context);
    let mut sni = config
        .tls
        .sni
        .write()
        .unwrap_or_else(PoisonError::into_inner);
    if !sni.iter().any(|(stored, _)| stored == name) {
        sni.push((name.to_vec(), Arc::new(settings)));
    }
    true
}

/// `ListenPort::ssl_init()`: the port's context (no files allowed when SNI
/// contexts exist), and the SNI callback when they do.
#[must_use]
pub fn ssl_init(config: &PortConfig) -> bool {
    let Some(context) = config.ssl_context.get() else {
        return false;
    };
    let has_sni = !sni(config).is_empty();
    let callback: swoole_rs_tls::ServernameCallback = sni_callback;
    let callback = has_sni.then_some(callback);
    let Some(settings) = context_create(config, &context, has_sni, callback) else {
        return false;
    };
    config.tls.settings.set(settings);
    true
}

/// `ListenPort::ssl_context_init()`: a new default context with upstream's flags
/// and ciphers; DTLS on a datagram port.
pub(crate) fn ssl_context_init(config: &mut PortConfig) -> bool {
    let context = TlsContext::with_texts(&[
        (Text::Ciphers, SW_SSL_CIPHER_LIST.as_bytes()),
        (Text::EcdhCurve, SW_SSL_ECDH_CURVE.as_bytes()),
    ]);
    config.ssl_context.set(context.clone());
    edit(&context, |fields| {
        fields.prefer_server_ciphers = true;
        fields.session_tickets = false;
        fields.stapling = true;
        fields.stapling_verify = true;
    });
    // crates/tls requires OpenSSL 3, which has DTLS.
    if matches!(config.r#type, 2 | 4 | 6) {
        let protocols = SslVersion::SW_SSL_DTLS.bits();
        edit(&context, |fields| fields.protocols = protocols);
    }
    true
}

/// `ListenPort::ssl_create(sock)`: the socket's server-side `SSL`, which names the port.
pub(crate) fn ssl_create(config: &PortConfig, sock: Option<Socket<'_>>) -> bool {
    let context = config.tls.settings.get();
    let create = |sock| {
        context
            .as_ref()
            .is_some_and(|context| context.create_session(sock))
    };
    let Some(sock) = sock.filter(|&sock| create(sock)) else {
        log::set_last_error(SSL_CREATE_SESSION_FAILED);
        return false;
    };
    // The port outlives its connections; the SNI callback only reads it.
    let port = ptr::from_ref(config).cast_mut().cast();
    if !Ssl::of(sock).is_some_and(|ssl| ssl.get().set_ex_data(swr_v1_tls_port_index(), port)) {
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
    let Some(port) = unsafe { port.cast::<PortConfig>().as_ref() }.map(view::Port::from) else {
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
    context.select(ssl);
    TLSEXT_OK
}

/// `getsockopt(fd, SOL_SOCKET, option)`: the value, or `None` (errno set).
fn get_option(fd: c_int, option: c_int) -> Option<c_int> {
    let mut value: c_int = 0;
    swoole_rs_corebase::sys::getsockopt(fd, libc::SOL_SOCKET, option, &mut value).ok()?;
    Some(value)
}

/// `ListenPort::import(fd)`: adopts a listening socket; the fd stays the
/// caller's when it isn't one. On failure the port takes no record.
pub(crate) fn import(config: &mut PortConfig, fd: c_int) -> bool {
    let origin = c"ListenPort::import";
    let mut socket = SocketRecord::with_fd(fd, 0);
    let Some(so_type) = get_option(fd, libc::SO_TYPE) else {
        let message = format_args!("getsockopt({fd}, SOL_SOCKET, SO_TYPE) failed");
        log::sys_warning(origin, message);
        return false;
    };
    if !socket.get_name() {
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
    let kind = socket_type(family, so_type);
    socket.socket_type = kind.raw();
    socket.info.kind = kind.raw();
    socket.fd_type = if kind.is_dgram() {
        FD_DGRAM_SERVER
    } else {
        FD_STREAM_SERVER
    };
    socket.removed = true;
    let info = socket.info;
    config.set_listening_socket(OwnedSocket::new(Box::new(socket)));
    config.r#type = kind.raw();
    if !with_host(&info, |host| config.host.set(host.to_bytes())) {
        swoole_rs_corebase::fatal("out of memory storing a port's host");
    }
    config.port.set(address_port(&info).into());
    config.listening.set(true);
    true
}

/// `ListenPort::close()`: drops the TLS context, frees the socket
/// and unlinks a Unix socket's path.
pub(crate) fn close(config: &PortConfig) {
    if config.ssl {
        drop(config.ssl_context.replace(None));
        drop(config.tls.settings.replace(None));
    }
    // Taken first, so nothing finds the record once it is freed.
    socket_free(config.take_socket());
    if matches!(config.r#type, 5 | 6) {
        let mut path = config.host_bytes().to_vec();
        path.push(0);
        if let Ok(path) = CStr::from_bytes_until_nul(&path) {
            _ = sys::unlink(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use swoole_rs_corebase::utilities::sni_wildcard_matches;

    #[test]
    fn the_tls_state_lives_in_the_port_until_freed() {
        let builder = swoole_rs_tls::TlsContext::new();
        let settings = builder.borrow_configuration().freeze(None).unwrap();
        let settings = std::sync::Arc::new(settings);
        let config = swoole_rs_server::port_config::swr_v1_port_config_new(0);
        let config = unsafe { &mut *config };
        assert_eq!(super::sni(config).len(), 0);
        config
            .tls
            .sni
            .write()
            .unwrap()
            .push((b"a".to_vec(), settings));
        assert_eq!(super::sni(config).len(), 1);
        super::tls_free(config);
        assert_eq!(super::sni(config).len(), 0);
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
