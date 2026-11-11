//! The `coroutine::Socket` TLS members: `ssl_context_create`, `ssl_create`,
//! `ssl_verify` and `ssl_handshake`. The socket's `SSLContext` owner is the one its extra record holds:
//! `enable_ssl_encrypt` makes it, and an accepted socket shares its server's.
use crate::co_io::{Timer, available, default_err, set_err};
use crate::co_socket::Co;
use crate::connect::{Handshaking, ssl_handshake};
use crate::socket_wait::wait_for;
use core::ffi::{CStr, c_int};
use swoole_rs_corebase::log;
use swoole_rs_tls::upstream::{
    socket_ssl_accept, socket_ssl_check_host, socket_ssl_connect, socket_ssl_verify,
};
use swoole_rs_tls::{
    OwnedTlsContext, owner_fields, owner_tls_host_name, reason_text, swoole_ssl_init,
    swr_v1_tls_owner_clone, swr_v1_tls_owner_new, swr_v1_tls_owner_socket_context,
    swr_v1_tls_owner_socket_session,
};

/// `SW_EVENT_READ` and `SW_EVENT_RDWR`.
const READ: u32 = 1 << 9;
const RDWR: u32 = READ | (1 << 10);
/// `SW_ERROR_SSL_VERIFY_FAILED`, `SW_ERROR_SSL_CREATE_CONTEXT_FAILED` and
/// `SW_ERROR_SSL_CREATE_SESSION_FAILED`.
const SSL_VERIFY_FAILED: c_int = 1014;
const SSL_CREATE_CONTEXT_FAILED: c_int = 1019;
const SSL_CREATE_SESSION_FAILED: c_int = 1020;

impl Co {
    /// The socket's `SSLContext` owner, or null without one.
    #[must_use]
    pub fn tls_owner(self) -> *const OwnedTlsContext {
        let extra = self.extra();
        if extra.is_null() {
            return core::ptr::null();
        }
        // SAFETY: the record's live extra.
        unsafe { (*extra).tls }
    }

    /// `Socket::enable_ssl_encrypt()`: a fresh `SSLContext` owner; false when the
    /// socket has one already.
    #[must_use]
    pub fn enable_ssl(self) -> bool {
        self.with_extra(|extra| {
            if !extra.tls.is_null() {
                return false;
            }
            extra.tls = swr_v1_tls_owner_new().owner;
            true
        })
        .unwrap_or(false)
    }

    /// The accepting constructor's TLS: `self`, accepted on `server`, shares the
    /// server's context and gets a session over it, or is closed when that fails.
    pub(crate) fn share_tls(self, server: Self) {
        let owner = server.tls_owner();
        if owner.is_null() {
            return;
        }
        // SAFETY: the server's live owner.
        let shared = swr_v1_tls_owner_clone(unsafe { &*owner });
        // The client's metadata takes the reference.
        self.with_extra(|extra| extra.tls = shared);
        if !self.create_session(owner) {
            // Nothing else holds the new socket yet.
            self.close();
        }
    }

    /// `Socket::ssl_context_create()`: the socket's context, configured for it.
    pub(crate) fn ssl_context_create(self) -> bool {
        swoole_ssl_init();
        // SAFETY: the live owner and network::Socket; TLS is initialised.
        let made = unsafe {
            swr_v1_tls_owner_socket_context(self.tls_owner(), self.socket(), self.http2(), true)
        };
        match made {
            1 => {
                log::warning(
                    c"Socket::ssl_context_create",
                    format_args!("DTLS support require openssl-1.1 or later"),
                );
                false
            }
            2 => {
                set_err(self, SSL_CREATE_CONTEXT_FAILED);
                false
            }
            _ => true,
        }
    }

    /// `Socket::ssl_create(context)`: the socket's TLS session over `owner`.
    fn create_session(self, owner: *const OwnedTlsContext) -> bool {
        // SAFETY: the record's live extra and its host name slot.
        let hostname = unsafe { &raw const (*self.extra()).strings[2] };
        // SAFETY: the live owner, network::Socket and slot; TLS is initialised.
        let result = unsafe { swr_v1_tls_owner_socket_session(owner, self.socket(), hostname) };
        let origin = c"Socket::ssl_create";
        match result.status {
            0 => return true,
            1 => log::warning(origin, format_args!("SSL_new() failed")),
            2 => log::warning(
                origin,
                format_args!(
                    "SSL_set_fd() failed. Error: {}[{}]",
                    reason_text(result.error),
                    result.error
                ),
            ),
            _ => log::warning(origin, format_args!("SSL_set_ex_data() failed")),
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
                // SAFETY: the live owner of the socket's context.
                let name = unsafe { owner_tls_host_name(self.tls_owner()) };
                name.length == 0 || {
                    // SAFETY: the view's NUL-terminated bytes, live for the call.
                    let name = unsafe { CStr::from_ptr(name.data) };
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
        let owner = self.tls_owner();
        let fields = if owner.is_null() {
            swoole_rs_tls::OwnedTlsFields::default()
        } else {
            // SAFETY: the live owner.
            unsafe { owner_fields(owner) }
        };
        let mut state = Handshake {
            socket: self,
            timer: Timer::new(self, READ, self.read_timeout()),
            handshaked: self.ssl_handshaked(),
            context_ready: !fields.context.is_null(),
            verify_peer: fields.verify_peer != 0,
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
        crate::uring_socket::of(self.socket).is_some()
    }

    /// The socket's session over its own context.
    fn create_session(&self) -> bool {
        self.socket.create_session(self.socket.tls_owner())
    }
}

impl Handshaking for Handshake {
    fn mode(&self) -> c_int {
        #[cfg(feature = "iouring")]
        if self.uring() {
            return 2;
        }
        c_int::from(self.socket.ssl_is_server())
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
            // SAFETY: the handshake's live socket; `create_session` makes its session.
            return unsafe {
                crate::uring_socket::tls_session(self.socket, || self.create_session())
            };
        }
        self.create_session()
    }

    /// A `UringSocket`'s step goes through the ring.
    fn step(&mut self) -> c_int {
        let socket = self.socket;
        #[cfg(feature = "iouring")]
        if self.uring() {
            // SAFETY: the handshake's live socket, with its session.
            return unsafe { crate::uring_socket::tls_step(socket) };
        }
        let server = socket.ssl_is_server();
        let step = socket.with_network(|network| {
            if server {
                socket_ssl_accept(network)
            } else {
                socket_ssl_connect(network)
            }
        });
        match step {
            None => return -1,
            Some(accepted) if server => return accepted,
            Some(connected) if connected < 0 => return -1,
            Some(_) => {}
        }
        let state = socket.net(|record| record.tls.state).unwrap_or_default();
        c_int::try_from(state).unwrap_or(c_int::MAX)
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
        // SAFETY: the live owner; verified only with a context.
        let fields = unsafe { owner_fields(self.socket.tls_owner()) };
        self.socket.ssl_verify(fields.allow_self_signed != 0)
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
