//! The `coroutine::Socket` TLS members: `ssl_context_create`, `ssl_create`,
//! `ssl_verify` and `ssl_handshake`. The extra record retains the configuration
//! builder and runtime snapshot; an accepted socket shares its server's snapshot.
use crate::co_io::{Timer, available, default_err, set_err};
use crate::co_socket::Co;
use crate::connect::{HandshakeMode, Handshaking, Step, ssl_handshake};
use crate::socket_wait::wait_for;
use core::ffi::c_int;
use std::sync::Arc;
use swoole_rs_corebase::log;
use swoole_rs_corebase::reactor::Events;
use swoole_rs_corebase::tls::SslCreateFlag::{SW_SSL_CLIENT as CLIENT, SW_SSL_SERVER as SERVER};
use swoole_rs_corebase::tls::upstream::{
    check_peer_name, socket_ssl_accept, socket_ssl_connect, socket_ssl_verify,
};
use swoole_rs_corebase::tls::{
    Accept, OwnedTlsContext, SocketContextError, SslNewError, TlsContext, TlsSettings,
    freeze_socket_context, socket_session,
};

/// `SW_ERROR_SSL_VERIFY_FAILED`, `SW_ERROR_SSL_CREATE_CONTEXT_FAILED` and
/// `SW_ERROR_SSL_CREATE_SESSION_FAILED`.
const SSL_VERIFY_FAILED: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SSL_VERIFY_FAILED as c_int;
const SSL_CREATE_CONTEXT_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SSL_CREATE_CONTEXT_FAILED as c_int;
const SSL_CREATE_SESSION_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_SSL_CREATE_SESSION_FAILED as c_int;

impl Co {
    /// The socket's `SSLContext` owner, or null without one.
    #[must_use]
    pub fn tls_owner(self) -> *const OwnedTlsContext {
        self.with_extra(|extra| extra.tls.as_ref().map(TlsContext::as_ptr))
            .flatten()
            .unwrap_or(core::ptr::null())
    }

    /// A reference of the caller's own to the socket's `SSLContext` owner, which keeps
    /// it live whatever happens to the socket; none without one.
    #[must_use]
    pub fn tls_context(self) -> Option<TlsContext> {
        self.with_extra(|extra| extra.tls.clone()).flatten()
    }

    /// The immutable configuration used by this socket and its accepted children.
    pub(crate) fn tls_settings(self) -> Option<Arc<TlsSettings>> {
        self.with_extra(|extra| extra.tls_settings.clone())
            .flatten()
    }

    /// The session's explicit name, or the builder's name before a handshake
    /// (used while negotiating a proxy).
    pub(crate) fn tls_host_name(self) -> Vec<u8> {
        self.tls_settings().map_or_else(
            || {
                self.tls_context().map_or_else(Vec::new, |owner| {
                    swoole_rs_corebase::tls::owner_host_name(&owner)
                })
            },
            |settings| settings.host_name().to_bytes().to_vec(),
        )
    }

    /// `Socket::enable_ssl_encrypt()`: a fresh `SSLContext` owner; false when the
    /// socket has one already.
    #[must_use]
    pub fn enable_ssl(self) -> bool {
        self.with_extra(|extra| {
            if extra.tls.is_some() {
                return false;
            }
            extra.tls = Some(TlsContext::new());
            true
        })
        .unwrap_or(false)
    }

    /// The accepting constructor's TLS: `self`, accepted on `server`, shares the
    /// server's context and gets a session over it, or is closed when that fails.
    pub(crate) fn share_tls(self, server: Self) {
        let Some(shared) = server.tls_context() else {
            return;
        };
        // The client's metadata takes a reference.
        let settings = server.tls_settings();
        self.with_extra(|extra| {
            extra.tls = Some(shared);
            extra.tls_settings.clone_from(&settings);
        });
        if !self.create_session(settings.as_deref()) {
            // Nothing else holds the new socket yet.
            self.close();
        }
    }

    /// `Socket::ssl_context_create()`: the socket's context, configured for it.
    pub(crate) fn ssl_context_create(self) -> bool {
        let (owner, http2) = (self.tls_context(), self.http2());
        let made = owner.and_then(|owner| {
            self.with_network(|socket| {
                freeze_socket_context(&mut owner.borrow_configuration(), socket, http2, true)
            })
        });
        match made.unwrap_or(Err(SocketContextError::Create)) {
            Ok(settings) => self
                .with_extra(|extra| extra.tls_settings = Some(Arc::new(settings)))
                .is_some(),
            Err(SocketContextError::UnsupportedDtls) => {
                log::warning(
                    c"Socket::ssl_context_create",
                    format_args!("DTLS support require openssl-1.1 or later"),
                );
                false
            }
            Err(SocketContextError::Create) => {
                set_err(self, SSL_CREATE_CONTEXT_FAILED);
                false
            }
        }
    }

    /// `Socket::ssl_create(context)`: the socket's TLS session over `owner`. A
    /// `UringSocket`'s runs over the ring's memory BIOs, in its side's state.
    fn create_session(self, owner: Option<&TlsSettings>) -> bool {
        // The automatic host name, as C reads it, none when the slot is empty.
        let hostname = self.with_extra(|extra| {
            let slot = &extra.strings.as_ref()[2];
            (!slot.bytes().is_empty()).then(|| slot.c_str().to_owned())
        });
        let Some(hostname) = hostname else {
            set_err(self, SSL_CREATE_SESSION_FAILED);
            return false;
        };
        #[cfg(feature = "iouring")]
        let uring = self.with_extra(|extra| extra.uring.is_some()) == Some(true);
        #[cfg(not(feature = "iouring"))]
        let uring = false;
        let ring = uring.then(|| {
            let side = if self.ssl_is_server() { SERVER } else { CLIENT };
            side as c_int
        });
        let result =
            self.with_network(|socket| socket_session(owner, socket, hostname.as_deref(), ring));
        match result.unwrap_or(Err(SslNewError::New)) {
            Ok(()) => return true,
            Err(error) => error.warn(),
        }
        set_err(self, SSL_CREATE_SESSION_FAILED);
        false
    }

    /// `Socket::ssl_verify(allow_self_signed)`: the peer's certificate, and its host
    /// against the context's `tls_host_name` when one is set. The socket has an
    /// `SSLContext` and a TLS session.
    #[must_use]
    pub fn ssl_verify(self, allow_self_signed: bool) -> bool {
        let verified = self
            .with_network(|network| socket_ssl_verify(network, allow_self_signed))
            .unwrap_or(false)
            && self.verify_host();
        if !verified {
            set_err(self, SSL_VERIFY_FAILED);
        }
        verified
    }

    fn verify_host(self) -> bool {
        let policy = self.tls_settings().and_then(|s| s.verify_host());
        if policy == Some(false) {
            return true;
        }
        let strict = policy == Some(true);
        let mut name = self.tls_host_name();
        if name.is_empty() && strict {
            name = self
                .with_extra(|e| e.strings.as_ref()[2].bytes().to_vec())
                .unwrap_or_default();
        }
        if name.is_empty() && !strict {
            return true;
        }
        let verified = std::ffi::CString::new(name).is_ok_and(|name| {
            !name.to_bytes().is_empty()
                && self.with_network(|s| check_peer_name(s, &name, strict)) == Some(true)
        });
        if !verified {
            set_err(self, SSL_VERIFY_FAILED);
        }
        verified
    }

    /// The `network::Socket`'s read timeout, or 0 without one.
    pub(crate) fn read_timeout(self) -> f64 {
        self.net(|record| record.io_timeout(false)).unwrap_or(0.0)
    }

    /// `Socket::ssl_handshake()`: Rust's sequence ([`crate::connect::ssl_handshake`]) over
    /// this socket, then the socket is handshaked. In a coroutine.
    #[must_use]
    pub fn ssl_handshake(self) -> bool {
        let mut state = Handshake {
            socket: self,
            timer: Timer::new(self, Events::READ, self.read_timeout()),
            handshaked: self.ssl_handshaked(),
        };
        let done = ssl_handshake(&mut state);
        state.timer.stop();
        if done {
            self.set_ssl_handshaked(true);
        }
        done
    }
}

/// What `ssl_handshake`'s steps act on: the socket, the handshake's timer and the
/// socket's TLS state when it began.
struct Handshake {
    socket: Co,
    timer: Timer,
    handshaked: bool,
}

impl Handshake {
    /// Whether the socket is a `UringSocket`, whose handshake runs through the ring.
    #[cfg(feature = "iouring")]
    fn uring(&self) -> bool {
        crate::uring_socket::with(self.socket, |_| ()).is_some()
    }

    /// The socket's session over its own context.
    fn create_session(&self) -> bool {
        self.socket
            .create_session(self.socket.tls_settings().as_deref())
    }
}

impl Handshaking for Handshake {
    fn mode(&self) -> HandshakeMode {
        #[cfg(feature = "iouring")]
        if self.uring() {
            return HandshakeMode::SelfDriven;
        }
        if self.socket.ssl_is_server() {
            HandshakeMode::Server
        } else {
            HandshakeMode::Client
        }
    }

    fn handshaked(&self) -> bool {
        self.handshaked
    }

    fn context_ready(&self) -> bool {
        self.socket.tls_settings().is_some()
    }

    fn verification_required(&self) -> bool {
        self.socket
            .tls_settings()
            .is_some_and(|settings| settings.verify_peer() || settings.verify_host() == Some(true))
    }

    fn available(&mut self) -> bool {
        available(self.socket, Events::RDWR)
    }

    fn set_err(&mut self, error: c_int) {
        if error < 0 {
            default_err(self.socket, log::last_error());
        } else {
            set_err(self.socket, error);
        }
    }

    fn context_create(&mut self) -> bool {
        self.socket.ssl_context_create()
    }

    fn create(&mut self) -> bool {
        self.create_session()
    }

    /// A `UringSocket`'s step goes through the ring.
    fn step(&mut self) -> Step {
        let socket = self.socket;
        #[cfg(feature = "iouring")]
        if self.uring() {
            return crate::uring_socket::tls_step(socket);
        }
        let server = socket.ssl_is_server();
        let step = socket.with_network(|network| {
            if server {
                Ok(socket_ssl_accept(network))
            } else {
                Err(socket_ssl_connect(network))
            }
        });
        match step {
            None | Some(Ok(Accept::Error) | Err(false)) => Step::Failed,
            Some(Ok(Accept::Ready)) => Step::Ready,
            Some(Ok(Accept::Wait)) => Step::Wait,
            Some(Err(true)) => socket
                .net(|record| {
                    if record.tls_ready() {
                        Step::Ready
                    } else if record.tls_awaits_stream() {
                        Step::Wait
                    } else {
                        Step::Again
                    }
                })
                .unwrap_or(Step::Again),
        }
    }

    fn wait(&mut self, fresh: bool) -> bool {
        let socket = self.socket;
        let waited = |timer: &mut Timer| timer.start() && wait_for(socket, Events::READ);
        if !fresh {
            return waited(&mut self.timer);
        }
        let mut timer = Timer::new(socket, Events::READ, socket.read_timeout());
        let ready = waited(&mut timer);
        timer.stop();
        ready
    }

    fn verify(&mut self) -> bool {
        self.socket.tls_settings().is_some_and(|settings| {
            if settings.verify_peer() {
                self.socket.ssl_verify(settings.allow_self_signed())
            } else {
                self.socket.verify_host()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::co_socket::TestSocket;

    #[test]
    fn enabling_tls_makes_one_owner_which_an_accepted_socket_needs_a_server_owner_to_share() {
        let (mut server, mut client) = (TestSocket::new(), TestSocket::new());
        client.co().share_tls(server.co());
        assert!(client.co().tls_owner().is_null());
        assert!(server.co().enable_ssl());
        let made = server.co().tls_owner();
        assert!(!made.is_null());
        assert!(!server.co().enable_ssl());
        assert_eq!(server.co().tls_owner(), made);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no OPENSSL_init_ssl")]
    fn accepted_socket_keeps_snapshot_across_listener_rebuild_and_drop() {
        use std::{os::fd::AsRawFd, sync::Arc};
        use swoole_rs_corebase::tls::Text;

        let (_peer, stream) = std::os::unix::net::UnixStream::pair().unwrap();
        let (mut server, mut client) = (TestSocket::new(), TestSocket::new());
        assert!(server.co().enable_ssl());
        let builder = server.co().tls_context().unwrap();
        assert!(
            builder
                .borrow_configuration()
                .set_text(Text::HostName, b"first.test")
        );
        assert!(server.co().ssl_context_create());
        let first = server.co().tls_settings().unwrap();
        client
            .co()
            .with_network(|socket| socket.edit(|fields| fields.fd = stream.as_raw_fd()));
        client.co().share_tls(server.co());
        assert!(Arc::ptr_eq(&first, &client.co().tls_settings().unwrap()));
        assert!(client.co().net(|fields| !fields.ssl().is_null()).unwrap());

        assert!(
            builder
                .borrow_configuration()
                .set_text(Text::HostName, b"second.test")
        );
        assert!(server.co().ssl_context_create());
        let second = server.co().tls_settings().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second.host_name(), c"second.test");
        drop(server);
        drop(builder);
        assert_eq!(client.co().tls_host_name(), b"first.test");
        client
            .co()
            .with_network(swoole_rs_corebase::tls::upstream::socket_ssl_close);
    }
}
