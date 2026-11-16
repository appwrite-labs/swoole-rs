//! The bundled `ext/ssh2`'s session socket (`--with-swoole-ssh2`; upstream's
//! `php_swoole_ssh2.h`): the coroutine wait crates/ssh2 retries libssh2 calls
//! over, the bound-coroutine check before each call, and the session timeout.
use crate::co_socket::Co;
use core::ffi::{c_int, c_void};
use core::marker::PhantomData;
use swoole_rs_reactor::Events;
use swoole_rs_ssh2_sys::LIBSSH2_SESSION;

/// `php_ssh2_session_data`: four userspace callbacks, then the `SocketImpl *`, the
/// session's live coroutine socket.
#[repr(C)]
struct SessionData {
    callbacks: [*mut c_void; 4],
    socket: Co,
}

/// An ext/ssh2 session live for `'a`, whose abstract is its session data. Only an
/// export's parameter makes one, where the C caller's contract stands for the
/// constructor, as it does for a `&T` parameter; crates/ssh2 passes its own `Session`,
/// the same pointer under the same contract, to the table's wait. Neither `Send` nor
/// `Sync`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Session<'a>(*mut LIBSSH2_SESSION, PhantomData<&'a LIBSSH2_SESSION>);

// libssh2's declaration, typed for the live session: it returns the address of the
// session's abstract slot, which holds ext/ssh2's session data (the type's
// invariant), and reads nothing else.
#[expect(
    improper_ctypes,
    reason = "the result is the slot's address; only Rust reads the session data behind \
              it, and C never calls the socket record's Rust callbacks through it"
)]
unsafe extern "C" {
    #[link_name = "libssh2_session_abstract"]
    safe fn session_abstract<'a>(session: Session<'a>) -> &'a &'a SessionData;
}

impl Session<'_> {
    /// `ssh2_get_socket`: the session's coroutine socket.
    fn socket(self) -> Co {
        session_abstract(self).socket
    }
}

/// `swr_v1_ssh2_ops`, as crates/ssh2 takes it.
#[repr(C)]
pub struct Ops {
    wait: extern "C" fn(session: Session<'_>, event: c_int) -> bool,
}

static OPS: Ops = Ops { wait };

/// `ssh2_wait`: park the coroutine until `event` on the session's socket. crates/ssh2
/// calls it in a coroutine.
extern "C" fn wait(session: Session<'_>, event: c_int) -> bool {
    session
        .socket()
        .poll(swoole_rs_reactor::Events::from_bits(event), 0.0)
}

/// `ssh2_bound`: the wait, once no other coroutine is bound to the session's socket.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ssh2_bound(session: Session<'_>) -> *const Ops {
    let socket = session.socket();
    socket.check_bound(Events::READ);
    socket.check_bound(Events::WRITE);
    &raw const OPS
}

/// `ssh2_set_socket_timeout`: whole seconds of `timeout_ms` for every kind.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ssh2_set_socket_timeout(session: Session<'_>, timeout_ms: c_int) {
    session.socket().set_timeout(
        f64::from(timeout_ms / 1000),
        swoole_rs_reactor::TimeoutKind::ALL,
    );
}
