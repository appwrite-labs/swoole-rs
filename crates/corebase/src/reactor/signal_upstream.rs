// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/signal.cc (Swoole contributors).
//! `swoole_signal_set`, `_callback`, `_get_handler`, `_get_listener_num` and `_clear`
//! as Rust operations over `corebase::process`'s signal loop, and the
//! event-loop effects that loop asks for: the signalfd socket, its reactor handler,
//! exit condition and destroy callback, and the warnings. The process patch deletes
//! `src/os/signal.cc`.
use crate::log;
pub use crate::process::signal::Handler;
use crate::process::signal::signal_name;
use crate::process::signal_loop::{self, Ops, Report};
use crate::reactor::ReactorPrepared;
use crate::reactor::socket_sync::set_errno;
use std::ffi::c_int;

/// `SW_FD_SIGNAL` and `SW_ERROR_UNREGISTERED_SIGNAL`.
#[cfg(target_os = "linux")]
const FD_SIGNAL: c_int = 9;
const UNREGISTERED_SIGNAL: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_UNREGISTERED_SIGNAL as c_int;

use crate::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;

/// The fields of the thread's event loop, available or not (`sw_reactor()`).
fn current() -> Option<ReactorPrepared> {
    crate::reactor::event_api::current_prepared()
}

fn capabilities() -> u8 {
    u8::from(cfg!(target_os = "linux"))
        | (u8::from(cfg!(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))) << 1)
}

fn available() -> bool {
    !crate::reactor::event_api::swr_v1_event_reactor().is_null()
}

fn has_reactor() -> bool {
    current().is_some()
}

fn native_handle() -> i32 {
    crate::reactor::event_api::field(|fields| fields.native_handle).unwrap_or(-1)
}

fn report(report: Report) {
    let unregistered = |origin, signo| {
        log::error_log(
            LOG_WARNING,
            UNREGISTERED_SIGNAL,
            origin,
            format_args!(
                "Unable to find callback function for signal {}",
                signal_name(signo).as_cstr().to_string_lossy()
            ),
        );
    };
    let failed = |error, origin, message: std::fmt::Arguments<'_>| {
        log::sys_warning_errno(error, origin, message);
        set_errno(error);
    };
    match report {
        Report::CallbackOutOfRange(signo) => log::warning(
            c"swoole_signal_callback",
            format_args!("signal[{signo}] numberis invalid"),
        ),
        Report::HandlerOutOfRange(signo) => log::warning(
            c"swoole_signal_get_handler",
            format_args!("signal[{signo}] numberis invalid"),
        ),
        Report::CallbackUnregistered(signo) => unregistered(c"swoole_signal_callback", signo),
        Report::SignalfdFailed(error) => failed(
            error,
            c"swoole_signalfd_create",
            format_args!("signalfd() failed"),
        ),
        Report::MaskFailed(error) => failed(
            error,
            c"swoole_signalfd_create",
            format_args!("sigprocmask() failed"),
        ),
        Report::UnblockFailed(error) => failed(
            error,
            c"swoole_signalfd_clear",
            format_args!("sigprocmask(SIG_UNBLOCK) failed"),
        ),
        Report::ReadFailed(error) => failed(
            error,
            c"swoole_signalfd_event_callback",
            format_args!("read from signalfd failed"),
        ),
        Report::UnknownSignal(signo) => log::warning(
            c"swoole_signalfd_event_callback",
            format_args!("unknown signal[{signo}]"),
        ),
        Report::ReadUnregistered(signo) => unregistered(c"swoole_signalfd_event_callback", signo),
        Report::KeventFailed { signo, error } => failed(
            error,
            c"swoole_signal_kqueue_set",
            format_args!("kevent set signal[{signo}] error"),
        ),
    }
}

#[cfg(target_os = "linux")]
mod descriptor {
    use super::{FD_SIGNAL, OPS, c_int};
    use crate::process::signal_loop::DescriptorOps;
    use crate::reactor::event_api::{
        FdHandler, event_add, event_del, isset_handler, swoole_event_set_fd_handler,
    };
    use crate::reactor::{OwnedSocket, SocketRecord};
    use core::cell::Cell;

    std::thread_local! {
        /// The signalfd's socket wrapper, on the signal thread: none, or the one
        /// `make` created and `free` has not yet released.
        static SOCKET: Cell<Option<OwnedSocket>> = const { Cell::new(None) };
    }

    pub(super) fn make(fd: i32) {
        let socket = OwnedSocket::new(Box::new(SocketRecord::with_fd(fd, FD_SIGNAL)));
        SOCKET.set(Some(socket));
    }

    /// `f` over the wrapper, while there is one, taken out and put back: a call `f`
    /// makes back into this module finds none.
    fn with<R>(f: impl FnOnce(&OwnedSocket) -> R) -> Option<R> {
        let socket = SOCKET.take()?;
        let out = f(&socket);
        SOCKET.set(Some(socket));
        Some(out)
    }

    /// Frees the wrapper but not the descriptor, which the signalfd backend closes.
    pub(super) fn free() {
        let Some(socket) = SOCKET.take() else {
            return;
        };
        socket.socket().edit(|record| record.fd = -1);
        crate::tls::upstream::socket_free(Some(socket));
    }

    pub(super) fn events() -> i32 {
        with(|socket| socket.socket().read(|record| record.events.bits())).unwrap_or(0)
    }

    pub(super) fn add(events: i32) -> bool {
        with(|socket| event_add(socket.heap(), crate::reactor::Events::from_bits(events)))
            .unwrap_or(false)
    }

    pub(super) fn del() {
        with(|socket| _ = event_del(socket.socket()));
    }

    pub(super) fn installed() -> bool {
        isset_handler(FD_SIGNAL, crate::reactor::Events::READ)
    }

    /// The signalfd's read handler.
    struct Read;

    impl FdHandler for Read {
        fn handle(fd: c_int) -> bool {
            crate::process::signal_loop::read(fd, &OPS);
            true
        }
    }

    fn destroyed() {
        crate::process::signal_loop::destroyed(&OPS);
    }

    /// The signalfd's exit condition, which the reactor calls with its event count: it
    /// discounts the wrapper while there is one. A freed wrapper leaves the registry
    /// (`OwnedSocket::into_box`), where upstream's stayed counted, so it no longer
    /// needs discounting.
    fn exit_condition(event_num: &mut usize) -> bool {
        with(|_| ()).is_none() || crate::process::signal_loop::exit_condition(event_num)
    }

    /// The read handler, the exit condition and the destroy callback.
    pub(super) fn install() {
        swoole_event_set_fd_handler::<Read>(FD_SIGNAL, crate::reactor::Events::READ);
        crate::reactor::swr_v1_event_set_exit_condition(
            crate::reactor::ExitCondition::Signalfd,
            exit_condition,
        );
        crate::reactor::event_api::on_destroy(destroyed);
    }

    pub(super) fn erase_end() {
        crate::reactor::swr_v1_event_erase_end_callback(crate::reactor::Priority::SignalCallback);
    }

    pub(super) static OPS_DESCRIPTOR: DescriptorOps = DescriptorOps {
        make,
        free,
        events,
        add,
        del,
        installed,
        install,
        erase_end,
    };
}

static OPS: Ops = Ops {
    capabilities,
    available,
    has_reactor,
    native_handle,
    callback: swoole_signal_callback,
    #[cfg(target_os = "linux")]
    descriptor: Some(&descriptor::OPS_DESCRIPTOR),
    #[cfg(not(target_os = "linux"))]
    descriptor: None,
    report,
};

/// `swoole_signal_set(signo, handler, safety)`: the handler it replaced.
#[must_use]
pub fn set(signo: c_int, handler: Handler, safety: bool) -> Handler {
    signal_loop::set(signo, handler, safety, &OPS)
}

/// `swoole_signal_get_handler(signo)`.
#[must_use]
pub fn get(signo: c_int) -> Handler {
    signal_loop::get(signo, &OPS)
}

/// `swoole_signal_clear()`.
pub fn clear() {
    signal_loop::clear(&OPS);
}

/// `void swoole_signal_callback(int signo)`: runs the handler registered for `signo`,
/// which [`Handler::new`] or C++'s `SignalHandler` made sound to call with any
/// signal number.
pub extern "C" fn swoole_signal_callback(signo: c_int) {
    signal_loop::callback(signo, &OPS);
}

/// `SignalHandler swoole_signal_get_handler(int signo)`.
#[must_use]
pub extern "C" fn swoole_signal_get_handler(signo: c_int) -> usize {
    get(signo).raw()
}

/// `uint32_t swoole_signal_get_listener_num()`.
#[must_use]
pub extern "C" fn swoole_signal_get_listener_num() -> u32 {
    signal_loop::listener_num()
}

/// `void swoole_signal_clear()`.
pub extern "C" fn swoole_signal_clear() {
    clear();
}
