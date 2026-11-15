//! The `coroutine::Socket` TLS members: `ssl_context_create`, `ssl_create`,
//! `ssl_verify` and `ssl_handshake`. The socket's `SSLContext` owner is the one its extra record holds:
//! `enable_ssl_encrypt` makes it, and an accepted socket shares its server's.
use crate::co_io::{Timer, available, default_err, set_err};
use crate::co_socket::Co;
use crate::connect::{HandshakeMode, Handshaking, Step, ssl_handshake};
use crate::socket_wait::wait_for;
use core::ffi::{CStr, c_int};
use swoole_rs_corebase::log;
use swoole_rs_tls::upstream::{
    socket_ssl_accept, socket_ssl_check_host, socket_ssl_connect, socket_ssl_verify,
};
use swoole_rs_tls::{
    Accept, Lent, OwnedTlsContext, OwnedTlsFields, SocketContextError, SslNewError, SslState,
    TlsContext, owner_fields, owner_host_name, swoole_ssl_init, swr_v1_tls_owner_socket_context,
    swr_v1_tls_owner_socket_session,
};

/// `SW_EVENT_READ` and `SW_EVENT_RDWR`.
const READ: u32 = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as u32;
const RDWR: u32 = READ | swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as u32;
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

    /// `read` over the `SSLContext` owner's fields, or none without one.
    pub(crate) fn tls_fields<R>(self, read: impl FnOnce(&OwnedTlsFields) -> R) -> Option<R> {
        let owner = self.tls_context()?;
        // SAFETY: the owner's fields, which nothing writes meanwhile.
        Some(read(unsafe { owner_fields(&owner) }))
    }

    /// The `SSLContext` owner's `tls_host_name`, empty without one.
    pub(crate) fn tls_host_name(self) -> Vec<u8> {
        let Some(owner) = self.tls_context() else {
            return Vec::new();
        };
        owner_host_name(&owner).to_vec()
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
        self.with_extra(|extra| extra.tls = Some(shared.clone()));
        if !self.create_session(Some(&shared)) {
            // Nothing else holds the new socket yet.
            self.close();
        }
    }

    /// `Socket::ssl_context_create()`: the socket's context, configured for it.
    pub(crate) fn ssl_context_create(self) -> bool {
        swoole_ssl_init();
        let (owner, http2) = (self.tls_context(), self.http2());
        let made = owner.and_then(|owner| {
            self.with_network(|socket| {
                // SAFETY: the socket's owner, which only it configures, and only here.
                swr_v1_tls_owner_socket_context(unsafe { Lent::new(&owner) }, socket, http2, true)
            })
        });
        match made.unwrap_or(Err(SocketContextError::Create)) {
            Ok(()) => true,
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

    /// `Socket::ssl_create(context)`: the socket's TLS session over `owner`.
    fn create_session(self, owner: Option<&OwnedTlsContext>) -> bool {
        // The automatic host name, as C reads it, none when the slot is empty.
        let hostname = self.with_extra(|extra| {
            let slot = &extra.strings[2];
            (!slot.bytes().is_empty()).then(|| slot.c_str().to_owned())
        });
        let Some(hostname) = hostname else {
            set_err(self, SSL_CREATE_SESSION_FAILED);
            return false;
        };
        let result = self.with_network(|socket| {
            swr_v1_tls_owner_socket_session(
                // SAFETY: the owner, which only the socket configures.
                owner.map(|owner| unsafe { Lent::new(owner) }),
                socket,
                hostname.as_deref(),
            )
        });
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
            && {
                let mut name = self.tls_host_name();
                name.is_empty() || {
                    name.push(0);
                    let name = CStr::from_bytes_until_nul(&name).unwrap_or_default();
                    self.with_network(|network| socket_ssl_check_host(network, name))
                        .unwrap_or(false)
                }
            };
        if !verified {
            set_err(self, SSL_VERIFY_FAILED);
        }
        verified
    }

    /// The `network::Socket`'s read timeout, or 0 without one.
    pub(crate) fn read_timeout(self) -> f64 {
        self.net(|record| record.timeouts.read).unwrap_or(0.0)
    }

    /// `Socket::ssl_handshake()`: Rust's sequence ([`crate::connect::ssl_handshake`]) over
    /// this socket, then the socket is handshaked. In a coroutine.
    #[must_use]
    pub fn ssl_handshake(self) -> bool {
        let (context_ready, verify_peer) = self
            .tls_fields(|fields| (fields.context.is_some(), fields.verify_peer))
            .unwrap_or_default();
        let mut state = Handshake {
            socket: self,
            timer: Timer::new(self, READ, self.read_timeout()),
            handshaked: self.ssl_handshaked(),
            context_ready,
            verify_peer,
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
    context_ready: bool,
    verify_peer: bool,
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
            .create_session(self.socket.tls_context().as_deref())
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
        self.context_ready
    }

    fn verify_peer(&self) -> bool {
        self.verify_peer
    }

    fn available(&mut self) -> bool {
        available(self.socket, RDWR)
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

    /// A `UringSocket`'s session is over the ring's memory BIOs.
    fn create(&mut self) -> bool {
        #[cfg(feature = "iouring")]
        if self.uring() {
            return crate::uring_socket::tls_session(self.socket, || self.create_session());
        }
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
            Some(Err(true)) => match socket.net(|record| record.tls.state) {
                Some(SslState::SW_SSL_STATE_READY) => Step::Ready,
                Some(SslState::SW_SSL_STATE_WAIT_STREAM) => Step::Wait,
                _ => Step::Again,
            },
        }
    }

    fn wait(&mut self, fresh: bool) -> bool {
        let socket = self.socket;
        let waited = |timer: &mut Timer| timer.start() && wait_for(socket, READ);
        if !fresh {
            return waited(&mut self.timer);
        }
        let mut timer = Timer::new(socket, READ, socket.read_timeout());
        let ready = waited(&mut timer);
        timer.stop();
        ready
    }

    fn verify(&mut self) -> bool {
        // Verified only with a context.
        let allow_self_signed = self.socket.tls_fields(|fields| fields.allow_self_signed);
        self.socket
            .ssl_verify(allow_self_signed.unwrap_or_default())
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
}
