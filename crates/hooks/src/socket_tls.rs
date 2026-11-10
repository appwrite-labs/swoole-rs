//! The `coroutine::Socket` TLS members: `ssl_context_create`, `ssl_create`,
//! `ssl_verify` and `ssl_handshake`. The socket's `SSLContext` owner is the one its extra record holds:
//! `enable_ssl_encrypt` makes it, and an accepted socket shares its server's.
use crate::co_io::{Timer, available, default_err, set_err};
use crate::co_socket::Co;
use crate::connect::{TlsOps, ssl_handshake};
use crate::socket_wait::wait_for;
use core::ffi::{c_int, c_void};
use swoole_rs_corebase::log;
use swoole_rs_tls::upstream::{
    socket_ssl_accept, socket_ssl_check_host, socket_ssl_connect, socket_ssl_verify,
};
use swoole_rs_tls::{
    OwnedTlsContext, owner_fields, owner_tls_host_name, reason_text, swoole_ssl_init,
    swr_v1_tls_owner_clone, swr_v1_tls_owner_new, swr_v1_tls_owner_socket_context,
    swr_v1_tls_owner_socket_session, swr_v1_tls_warning,
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
        // SAFETY: the record's live extra record.
        let tls = unsafe { &mut (*self.extra()).tls };
        if !tls.is_null() {
            return false;
        }
        *tls = swr_v1_tls_owner_new(swr_v1_tls_warning).owner;
        true
    }

    /// The accepting constructor's TLS: `self`, accepted on `server`, shares the
    /// server's context and gets a session over it, or is closed when that fails.
    pub(crate) fn share_tls(self, server: Self) {
        let owner = server.tls_owner();
        if owner.is_null() {
            return;
        }
        // SAFETY: the server's live owner.
        let shared = unsafe { swr_v1_tls_owner_clone(owner) };
        // SAFETY: the client's live extra record, which takes the reference.
        unsafe { (*self.extra()).tls = shared };
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
        let result =
            unsafe { swr_v1_tls_owner_socket_session(owner, self.socket(), hostname, None) };
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
        let network = self.socket();
        // SAFETY: the live network::Socket and its session.
        let verified = unsafe { socket_ssl_verify(network, allow_self_signed) } && {
            // SAFETY: the live owner of the socket's context.
            let name = unsafe { owner_tls_host_name(self.tls_owner()) };
            // SAFETY: the view's NUL-terminated bytes, live for the call.
            name.length == 0 || unsafe { socket_ssl_check_host(network, name.data) }
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
        let timer = Timer::new(self, READ, self.read_timeout());
        let mut state = Handshake {
            socket: self,
            timer,
        };
        let owner = self.tls_owner();
        let fields = if owner.is_null() {
            swoole_rs_tls::OwnedTlsFields::default()
        } else {
            // SAFETY: the live owner.
            unsafe { owner_fields(owner) }
        };
        let (mode, create, step) = steps(self);
        let ops = TlsOps {
            socket: (&raw mut state).cast(),
            mode,
            handshaked: self.ssl_handshaked(),
            context_ready: !fields.context.is_null(),
            verify_peer: fields.verify_peer != 0,
            available: tls_available,
            set_err: tls_set_err,
            context_create: tls_context_create,
            create,
            step,
            wait: tls_wait,
            verify: tls_verify,
        };
        let done = ssl_handshake(&ops);
        state.timer.stop();
        if done {
            self.set_ssl_handshaked(true);
        }
        done
    }
}

/// What `ssl_handshake`'s steps act on: the socket and the handshake's timer.
struct Handshake {
    socket: Co,
    timer: Timer,
}

/// # Safety
/// `context` is the live [`Handshake`] of the call.
unsafe fn handshake<'a>(context: *mut c_void) -> &'a mut Handshake {
    // SAFETY: as the caller promises.
    unsafe { &mut *context.cast::<Handshake>() }
}

/// The handshake's socket.
///
/// # Safety
/// As [`handshake`].
unsafe fn handshake_socket(context: *mut c_void) -> Co {
    // SAFETY: as the caller promises.
    unsafe { handshake(context) }.socket
}

unsafe extern "C" fn tls_available(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket.
    available(unsafe { handshake_socket(context) }, RDWR)
}

unsafe extern "C" fn tls_set_err(context: *mut c_void, error: c_int) {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    if error < 0 {
        default_err(socket, log::last_error());
    } else {
        set_err(socket, error);
    }
}

unsafe extern "C" fn tls_context_create(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket, which has a context.
    unsafe { handshake_socket(context) }.ssl_context_create()
}

unsafe extern "C" fn tls_create(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    socket.create_session(socket.tls_owner())
}

unsafe extern "C" fn tls_step(context: *mut c_void) -> c_int {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    let Some(network) = socket.network() else {
        return -1;
    };
    if socket.ssl_is_server() {
        return socket_ssl_accept(network);
    }
    if socket_ssl_connect(network) < 0 {
        return -1;
    }
    let state = socket.net(|record| record.tls.state).unwrap_or_default();
    c_int::try_from(state).unwrap_or(c_int::MAX)
}

/// A handshake step's callback.
type Step<T> = unsafe extern "C" fn(*mut c_void) -> T;

/// The handshake's mode with its session and step callbacks; a `UringSocket`'s
/// run through the ring.
fn steps(socket: Co) -> (c_int, Step<bool>, Step<c_int>) {
    #[cfg(feature = "iouring")]
    if crate::uring_socket::of(socket).is_some() {
        return (2, uring_create, uring_step);
    }
    (c_int::from(socket.ssl_is_server()), tls_create, tls_step)
}

/// A `UringSocket`'s session, over the ring's memory BIOs.
#[cfg(feature = "iouring")]
unsafe extern "C" fn uring_create(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    // SAFETY: the handshake's live socket.
    let create = || unsafe { tls_create(context) };
    // SAFETY: as above; `create` makes its session.
    unsafe { crate::uring_socket::tls_session(socket, create) }
}

/// A `UringSocket`'s step, through the ring.
#[cfg(feature = "iouring")]
unsafe extern "C" fn uring_step(context: *mut c_void) -> c_int {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    // SAFETY: as above, with its session.
    unsafe { crate::uring_socket::tls_step(socket) }
}

unsafe extern "C" fn tls_wait(context: *mut c_void, fresh: bool) -> bool {
    // SAFETY: the handshake of the call.
    let state = unsafe { handshake(context) };
    let socket = state.socket;
    let waited = |timer: &mut Timer| timer.start() && wait_for(socket, READ);
    if !fresh {
        return waited(&mut state.timer);
    }
    let mut timer = Timer::new(socket, READ, socket.read_timeout());
    let ready = waited(&mut timer);
    timer.stop();
    ready
}

unsafe extern "C" fn tls_verify(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    // SAFETY: the live owner; verified only with a context.
    let fields = unsafe { owner_fields(socket.tls_owner()) };
    socket.ssl_verify(fields.allow_self_signed != 0)
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
