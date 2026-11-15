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

/// A live ext/ssh2 session whose abstract is its session data, for as long as the
/// value is used. crates/ssh2 passes its own `Session`, the same pointer under the
/// same contract, to the table's wait; the exports make one from their caller's.
/// Neither `Send` nor `Sync`.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Session(*mut LIBSSH2_SESSION);

impl Session {
    /// # Safety
    /// `session` is a live ext/ssh2 session, its abstract its session data, for as
    /// long as the value is used.
    const unsafe fn new(session: *mut LIBSSH2_SESSION) -> Self {
        Self(session)
    }

    /// `ssh2_get_socket`: the session's coroutine socket.
    fn socket(self) -> Co {
        // SAFETY: a live session (the type's invariant).
        let data = unsafe { libssh2_session_abstract(self.0) }.cast::<*mut SessionData>();
        // SAFETY: ext/ssh2 stores its session data's address there.
        let data = unsafe { *data };
        // SAFETY: live session data.
        let socket = unsafe { (*data).socket };
        // SAFETY: the session's live coroutine socket.
        unsafe { Co::new(socket.cast()) }
    }
}

/// `swr_v1_ssh2_ops`, as crates/ssh2 takes it.
#[repr(C)]
pub struct Ops {
    wait: extern "C" fn(session: Session, event: c_int) -> bool,
}

static OPS: Ops = Ops { wait };

/// `ssh2_wait`: park the coroutine until `event` on the session's socket. crates/ssh2
/// calls it in a coroutine.
extern "C" fn wait(session: Session, event: c_int) -> bool {
    session.socket().poll(event.cast_unsigned(), 0.0)
}

/// `ssh2_bound`: the wait, once no other coroutine is bound to the session's socket.
///
/// # Safety
/// `session` is a live ext/ssh2 session, its abstract its session data.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ssh2_bound(session: *mut LIBSSH2_SESSION) -> *const Ops {
    // SAFETY: as the caller promises.
    let socket = unsafe { Session::new(session) }.socket();
    socket.check_bound(READ);
    socket.check_bound(WRITE);
    &raw const OPS
}

/// `ssh2_set_socket_timeout`: whole seconds of `timeout_ms` for every kind.
///
/// # Safety
/// As [`swr_v1_ssh2_bound`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ssh2_set_socket_timeout(
    session: *mut LIBSSH2_SESSION,
    timeout_ms: c_int,
) {
    // SAFETY: as the caller promises.
    unsafe { Session::new(session) }.socket().set_timeout(
        f64::from(timeout_ms / 1000),
        swoole_rs_reactor::TimeoutKind::ALL,
    );
}
