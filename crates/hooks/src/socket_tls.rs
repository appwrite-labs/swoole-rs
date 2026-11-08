//! The `coroutine::Socket` TLS members: `ssl_context_create`, `ssl_create`,
//! `ssl_verify` and `ssl_handshake`. The socket's `SSLContext` owner is the one its extra record holds:
//! `enable_ssl_encrypt` makes it, and an accepted socket shares its server's.
use crate::co_io::{Timer, available, set_err, swr_v1_hook_co_socket_default_err};
use crate::co_socket::CoSocket;
use crate::connect::{TlsOps, swr_v1_hook_ssl_handshake};
use crate::lifecycle::swr_v1_hook_co_socket_close;
use crate::socket_members::record;
use crate::socket_wait::wait;
use core::ffi::{c_int, c_void};
use core::ptr::null_mut;
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

/// The socket's `SSLContext` owner, or null without one.
///
/// # Safety
/// `socket` is a live record.
pub unsafe fn owner(socket: *mut CoSocket) -> *const OwnedTlsContext {
    // SAFETY: as the caller promises.
    let extra = unsafe { (*socket).extra };
    if extra.is_null() {
        return core::ptr::null();
    }
    // SAFETY: the record's live extra.
    unsafe { (*extra).tls }
}

/// `Socket::enable_ssl_encrypt()`: a fresh `SSLContext` owner; false when the
/// socket has one already.
///
/// # Safety
/// `socket` is a live record whose extra record is set.
pub unsafe fn enable_ssl(socket: *mut CoSocket) -> bool {
    // SAFETY: as the caller promises.
    let extra = unsafe { (*socket).extra };
    // SAFETY: the record's live extra record.
    let tls = unsafe { &mut (*extra).tls };
    if !tls.is_null() {
        return false;
    }
    *tls = swr_v1_tls_owner_new(swr_v1_tls_warning).owner;
    true
}

/// The accepting constructor's TLS: `client`, accepted on `server`, shares the
/// server's context and gets a session over it, or is closed when that fails.
///
/// # Safety
/// `server` and `client` are live records whose extra records are set.
pub(crate) unsafe fn share(server: *mut CoSocket, client: *mut CoSocket) {
    // SAFETY: as the caller promises.
    let owner = unsafe { owner(server) };
    if owner.is_null() {
        return;
    }
    // SAFETY: the server's live owner.
    let shared = unsafe { swr_v1_tls_owner_clone(owner) };
    // SAFETY: as the caller promises.
    let extra = unsafe { (*client).extra };
    // SAFETY: the client's live extra record, which takes the reference.
    unsafe { (*extra).tls = shared };
    // SAFETY: the live record and the context it now shares.
    if !unsafe { create_session(client, owner) } {
        // SAFETY: the live record, which nothing else holds yet.
        unsafe { swr_v1_hook_co_socket_close(client) };
    }
}

/// `Socket::ssl_context_create()`: the socket's context, configured for it.
///
/// # Safety
/// `this` is a live native socket with an `SSLContext`.
pub(crate) unsafe fn ssl_context_create(this: *mut c_void) -> bool {
    let socket = record(this);
    // SAFETY: as the caller promises.
    let owner = unsafe { owner(socket) };
    swoole_ssl_init();
    let (network, http2) = {
        // SAFETY: as the caller promises.
        let record = unsafe { &*socket };
        (record.socket, record.http2)
    };
    // SAFETY: the live owner and network::Socket; TLS is initialised.
    match unsafe { swr_v1_tls_owner_socket_context(owner, network, http2, true) } {
        1 => {
            log::warning(
                c"Socket::ssl_context_create",
                format_args!("DTLS support require openssl-1.1 or later"),
            );
            false
        }
        2 => {
            // SAFETY: the live record.
            unsafe { set_err(socket, SSL_CREATE_CONTEXT_FAILED) };
            false
        }
        _ => true,
    }
}

/// `Socket::ssl_create(context)`: the socket's TLS session over `owner`.
///
/// # Safety
/// `socket` is a live record and `owner` live.
unsafe fn create_session(socket: *mut CoSocket, owner: *const OwnedTlsContext) -> bool {
    // SAFETY: as the caller promises.
    let record = unsafe { &*socket };
    // SAFETY: the record's live extra and its host name slot.
    let hostname = unsafe { &raw const (*record.extra).strings[2] };
    let network = record.socket;
    // SAFETY: the live owner, network::Socket and slot; TLS is initialised.
    let result = unsafe { swr_v1_tls_owner_socket_session(owner, network, hostname, None) };
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
    // SAFETY: the live record.
    unsafe { set_err(socket, SSL_CREATE_SESSION_FAILED) };
    false
}

/// `Socket::ssl_verify(allow_self_signed)`: the peer's certificate, and its host
/// against the context's `tls_host_name` when one is set.
///
/// # Safety
/// `this` is a live native socket with an `SSLContext` and a TLS session. Its
/// upstream symbol is what tests/modules/hooks/system's socket.cpp port calls.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket10ssl_verifyEb")
)]
pub unsafe extern "C" fn ssl_verify(this: *mut c_void, allow_self_signed: bool) -> bool {
    let socket = record(this);
    // SAFETY: as the caller promises.
    let network = unsafe { (*socket).socket };
    // SAFETY: the live network::Socket and its session.
    let verified = unsafe { socket_ssl_verify(network, allow_self_signed) } && {
        // SAFETY: as the caller promises.
        let owner = unsafe { owner(socket) };
        // SAFETY: the live owner of the socket's context.
        let name = unsafe { owner_tls_host_name(owner) };
        // SAFETY: the view's NUL-terminated bytes, live for the call.
        name.length == 0 || unsafe { socket_ssl_check_host(network, name.data) }
    };
    if !verified {
        // SAFETY: the live record.
        unsafe { set_err(socket, SSL_VERIFY_FAILED) };
    }
    verified
}

/// What `ssl_handshake`'s steps act on: the socket and the handshake's timer.
struct Handshake {
    this: *mut c_void,
    timer: Timer,
}

/// # Safety
/// `context` is the live [`Handshake`] of the call.
unsafe fn handshake<'a>(context: *mut c_void) -> &'a mut Handshake {
    // SAFETY: as the caller promises.
    unsafe { &mut *context.cast::<Handshake>() }
}

/// The handshake's socket record.
///
/// # Safety
/// As [`handshake`].
unsafe fn handshake_socket(context: *mut c_void) -> *mut CoSocket {
    // SAFETY: as the caller promises.
    record(unsafe { handshake(context) }.this)
}

unsafe extern "C" fn tls_available(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    // SAFETY: as above.
    unsafe { available(socket, RDWR) }
}

unsafe extern "C" fn tls_set_err(context: *mut c_void, error: c_int) {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    if error < 0 {
        // SAFETY: the live record.
        unsafe { swr_v1_hook_co_socket_default_err(socket, log::last_error()) };
    } else {
        // SAFETY: the live record.
        unsafe { set_err(socket, error) };
    }
}

unsafe extern "C" fn tls_context_create(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket.
    let this = unsafe { handshake(context) }.this;
    // SAFETY: as above; it has a context.
    unsafe { ssl_context_create(this) }
}

unsafe extern "C" fn tls_create(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    // SAFETY: as above.
    let owner = unsafe { owner(socket) };
    // SAFETY: the live record and the owner of the context native code keeps.
    unsafe { create_session(socket, owner) }
}

unsafe extern "C" fn tls_step(context: *mut c_void) -> c_int {
    // SAFETY: the handshake's live socket.
    let socket = unsafe { handshake_socket(context) };
    // SAFETY: as above.
    let record = unsafe { &*socket };
    let (network, server) = (record.socket, record.ssl_is_server);
    if server {
        // SAFETY: the live network::Socket and its session.
        return unsafe { socket_ssl_accept(network) };
    }
    // SAFETY: as above.
    if unsafe { socket_ssl_connect(network) } < 0 {
        return -1;
    }
    // SAFETY: as above.
    c_int::try_from(unsafe { (*network).tls.state }).unwrap_or(c_int::MAX)
}

/// A handshake step's callback.
type Step<T> = unsafe extern "C" fn(*mut c_void) -> T;

/// The handshake's mode with its session and step callbacks; a `UringSocket`'s
/// run through the ring.
///
/// # Safety
/// `socket` is a live record.
unsafe fn steps(socket: *mut CoSocket) -> (c_int, Step<bool>, Step<c_int>) {
    #[cfg(feature = "iouring")]
    // SAFETY: as the caller promises.
    if unsafe { crate::uring_socket::of(socket) }.is_some() {
        return (2, uring_create, uring_step);
    }
    // SAFETY: as the caller promises.
    let server = unsafe { (*socket).ssl_is_server };
    (c_int::from(server), tls_create, tls_step)
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
    let socket = record(state.this);
    // SAFETY: the live record's own wait for the current coroutine.
    let waited =
        |timer: &mut Timer| timer.start() && unsafe { wait(socket, READ, None, null_mut(), 0) };
    if !fresh {
        return waited(&mut state.timer);
    }
    // SAFETY: the live record; the timer stops before this returns.
    let mut timer = unsafe { Timer::new(socket, READ, read_timeout(socket)) };
    let ready = waited(&mut timer);
    timer.stop();
    ready
}

unsafe extern "C" fn tls_verify(context: *mut c_void) -> bool {
    // SAFETY: the handshake's live socket.
    let this = unsafe { handshake(context) }.this;
    // SAFETY: as above.
    let owner = unsafe { owner(record(this)) };
    // SAFETY: the live owner; verified only with a context.
    let fields = unsafe { owner_fields(owner) };
    // SAFETY: as above.
    unsafe { ssl_verify(this, fields.allow_self_signed != 0) }
}

/// The `network::Socket`'s read timeout, or 0 without one.
pub(crate) fn read_timeout(socket: *mut CoSocket) -> f64 {
    // SAFETY: the caller's live record.
    let record = unsafe { &*socket };
    if record.socket.is_null() {
        return 0.0;
    }
    // SAFETY: the record's live network::Socket.
    unsafe { record.record() }.timeouts.read
}

/// `Socket::ssl_handshake()`: Rust's sequence (`swr_v1_hook_ssl_handshake`) over
/// this socket, then the socket is handshaked.
///
/// # Safety
/// `this` is a live native socket with an `SSLContext`, in a coroutine. Its
/// upstream symbol is what tests/modules/hooks/system's socket.cpp port calls.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole9coroutine6Socket13ssl_handshakeEv")
)]
pub unsafe extern "C" fn ssl_handshake(this: *mut c_void) -> bool {
    let socket = record(this);
    // SAFETY: as the caller promises; the timer stops before this returns.
    let timer = unsafe { Timer::new(socket, READ, read_timeout(socket)) };
    let mut state = Handshake { this, timer };
    // SAFETY: as the caller promises.
    let owner = unsafe { owner(socket) };
    let fields = if owner.is_null() {
        swoole_rs_tls::OwnedTlsFields::default()
    } else {
        // SAFETY: the live owner.
        unsafe { owner_fields(owner) }
    };
    // SAFETY: as the caller promises.
    let (mode, create, step) = unsafe { steps(socket) };
    let ops = TlsOps {
        socket: (&raw mut state).cast(),
        mode,
        // SAFETY: as above.
        handshaked: unsafe { (*socket).ssl_handshaked },
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
    // SAFETY: the table and its state are live for the call.
    let done = unsafe { swr_v1_hook_ssl_handshake(&raw const ops) };
    state.timer.stop();
    if done {
        // SAFETY: the live record.
        unsafe { (*socket).ssl_handshaked = true };
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_socket::TestSocket;
    use crate::socket_extra::{swr_v1_hook_socket_extra_free, swr_v1_hook_socket_extra_new};

    #[test]
    fn enabling_tls_makes_one_owner_which_an_accepted_socket_needs_a_server_owner_to_share() {
        let (mut server, mut client) = (TestSocket::new(), TestSocket::new());
        let extras = [
            swr_v1_hook_socket_extra_new(),
            swr_v1_hook_socket_extra_new(),
        ];
        server.0.extra = extras[0].owner;
        client.0.extra = extras[1].owner;
        unsafe { share(server.ptr(), client.ptr()) };
        assert!(unsafe { owner(client.ptr()) }.is_null());
        assert!(unsafe { enable_ssl(server.ptr()) });
        let made = unsafe { owner(server.ptr()) };
        assert!(!made.is_null());
        assert!(!unsafe { enable_ssl(server.ptr()) });
        assert_eq!(unsafe { owner(server.ptr()) }, made);
        for extra in extras {
            unsafe { swr_v1_hook_socket_extra_free(extra.owner) };
        }
    }
}
