//! The bundled `ext/ssh2`'s session socket (`--with-swoole-ssh2`; upstream's
//! `php_swoole_ssh2.h`): the coroutine wait crates/ssh2 retries libssh2 calls
//! over, the bound-coroutine check before each call, and the session timeout.
use crate::co_socket::Co;
use crate::shutdown::{READ, WRITE};
use core::ffi::{c_int, c_void};
use swoole_rs_ssh2_sys::{LIBSSH2_SESSION, libssh2_session_abstract};

/// `php_ssh2_session_data`: four userspace callbacks, then the `SocketImpl *`.
#[repr(C)]
struct SessionData {
    callbacks: [*mut c_void; 4],
    socket: *mut c_void,
}

/// `swr_v1_ssh2_ops`, as crates/ssh2 takes it.
#[repr(C)]
pub struct Ops {
    wait: unsafe extern "C" fn(session: *mut LIBSSH2_SESSION, event: c_int) -> bool,
}

static OPS: Ops = Ops { wait };

/// `ssh2_get_socket`: the session's coroutine socket.
///
/// # Safety
/// `session` is a live ext/ssh2 session, its abstract its session data.
unsafe fn socket(session: *mut LIBSSH2_SESSION) -> Co {
    // SAFETY: as the caller promises.
    let data = unsafe { libssh2_session_abstract(session) }.cast::<*mut SessionData>();
    // SAFETY: ext/ssh2 stores its session data's address there.
    let data = unsafe { *data };
    // SAFETY: live session data.
    let socket = unsafe { (*data).socket };
    // SAFETY: the session's live coroutine socket.
    unsafe { Co::new(socket.cast()) }
}

/// `ssh2_wait`: park the coroutine until `event` on the session's socket.
///
/// # Safety
/// As [`socket`], in a coroutine.
unsafe extern "C" fn wait(session: *mut LIBSSH2_SESSION, event: c_int) -> bool {
    // SAFETY: as the caller promises.
    unsafe { socket(session) }.poll(event.cast_unsigned(), 0.0)
}

/// `ssh2_bound`: the wait, once no other coroutine is bound to the session's socket.
///
/// # Safety
/// As [`socket`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ssh2_bound(session: *mut LIBSSH2_SESSION) -> *const Ops {
    // SAFETY: as the caller promises.
    let socket = unsafe { socket(session) };
    socket.check_bound(READ);
    socket.check_bound(WRITE);
    &raw const OPS
}

/// `ssh2_set_socket_timeout`: whole seconds of `timeout_ms` for every kind.
///
/// # Safety
/// As [`socket`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ssh2_set_socket_timeout(
    session: *mut LIBSSH2_SESSION,
    timeout_ms: c_int,
) {
    // SAFETY: as the caller promises.
    unsafe { socket(session) }.set_timeout(
        f64::from(timeout_ms / 1000),
        swoole_rs_reactor::TimeoutKind::ALL,
    );
}
