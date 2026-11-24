//! The handlers a reactor gets when it is created: the optional `io_uring` engine
//! and the readiness handlers of the layer that parks coroutines on the loop
//! (`crate::waits::readiness`), which it registers through [`set_init_reactor`].
use crate::reactor::{Handler, Running};
use core::ffi::c_int;

use std::sync::OnceLock;

/// The optional Linux ring backend. Corebase also runs on its own or with poll,
/// epoll or kqueue; the hooks crate links the ring implementation when enabled.
pub struct UringOps {
    /// The ring fd has completions ready to harvest.
    pub ready: Handler,
    /// Harvest completions from this thread's engine after epoll returns EINTR.
    pub interrupt: fn(),
    /// `Iouring::waitpid`.
    pub waitpid: fn(libc::pid_t, Option<&mut c_int>, c_int, f64) -> libc::pid_t,
}

/// The ring engine, if one is registered.
pub(crate) fn uring() -> Option<&'static UringOps> {
    INIT_REACTOR.get()?.uring
}

/// What each new reactor is initialized with.
struct Init {
    uring: Option<&'static UringOps>,
    init: fn(Running<'_>),
}
static INIT_REACTOR: OnceLock<Init> = OnceLock::new();

/// Install the ring engine and the upper layer's reactor initializer once, before
/// creating reactors.
pub fn set_init_reactor(uring: Option<&'static UringOps>, init: fn(Running<'_>)) {
    INIT_REACTOR.get_or_init(|| Init { uring, init });
}

/// Hands a new reactor to the initializer, if one is installed.
pub(crate) fn init_reactor(reactor: Running<'_>) {
    if let Some(init) = INIT_REACTOR.get() {
        (init.init)(reactor);
    }
}
