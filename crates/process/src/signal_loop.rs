// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/signal.cc (Swoole contributors).
//! Signal backend routing and signalfd reactor-wrapper custody. The existing
//! backends own handlers/masks/descriptors; SDK leaves lend reactor/socket effects.
use crate::signal::{self, Handler};
use swoole_rs_corebase as corebase;

/// The event loop's side of the signal backends (crates/reactor).
pub struct Ops {
    /// Bit 0: signalfd; bit 1: kqueue.
    pub capabilities: fn() -> u8,
    pub available: fn() -> bool,
    pub has_reactor: fn() -> bool,
    /// The available reactor's kqueue.
    pub native_handle: fn() -> i32,
    pub callback: signal::Dispatch,
    /// The signalfd socket's reactor effects (Linux).
    pub descriptor: Option<&'static DescriptorOps>,
    pub report: fn(Report),
}

/// A warning the event loop logs, with the signal number or errno it names. The
/// errno-carrying ones also set errno.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// `swoole_signal_callback` on an out-of-range signal.
    CallbackOutOfRange(i32),
    /// `swoole_signal_get_handler` on an out-of-range signal.
    HandlerOutOfRange(i32),
    /// `swoole_signal_callback` on a signal with no handler.
    CallbackUnregistered(i32),
    /// `signalfd()` failed.
    SignalfdFailed(i32),
    /// Blocking the signalfd's mask failed.
    MaskFailed(i32),
    /// Unblocking the signalfd's mask failed.
    UnblockFailed(i32),
    /// Reading the signalfd failed.
    ReadFailed(i32),
    /// The signalfd read an out-of-range signal.
    UnknownSignal(i32),
    /// The signalfd read a signal with no handler.
    ReadUnregistered(i32),
    /// Setting a signal on the kqueue failed.
    KeventFailed { signo: i32, error: i32 },
}

/// The signalfd socket's reactor effects. The reactor side keeps the socket
/// wrapper `make` creates until `free` releases it.
pub struct DescriptorOps {
    /// Wrap the signalfd's descriptor in a socket.
    pub make: fn(i32),
    /// Release the wrapper but not the descriptor, which the backend closes.
    pub free: fn(),
    /// The wrapper's events.
    pub events: fn() -> i32,
    /// Add the wrapper to the thread's loop for `events`: whether it worked.
    pub add: fn(i32) -> bool,
    /// Remove the wrapper, if any, from the loop.
    pub del: fn(),
    pub installed: fn() -> bool,
    pub install: fn(),
    pub erase_end: fn(),
}
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `swoole_signal_set` over the configured backend: the previous handler.
#[must_use]
pub fn set(signo: i32, handler: Handler, safety: bool, ops: &Ops) -> Handler {
    Handler(set_raw(signo, handler.0, safety, ops))
}

fn set_raw(signo: i32, handler: usize, safety: bool, ops: &Ops) -> usize {
    let caps = (ops.capabilities)();
    let available = (ops.available)();
    #[cfg(target_os = "linux")]
    if caps & 1 != 0 && corebase::swr_v1_core_enable_signalfd() && available {
        let previous = crate::signalfd::signalfd_set(signo, handler);
        setup(ops);
        return previous;
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    if caps & 2 != 0 && corebase::swr_v1_core_enable_kqueue() && available && signo != libc::SIGCHLD
    {
        let mut failed = false;
        let kq = (ops.native_handle)();
        let previous = crate::signal_kqueue::signal_kqueue_set(kq, signo, handler, &mut failed);
        if failed {
            (ops.report)(Report::KeventFailed {
                signo,
                error: errno(),
            });
        }
        return previous;
    }
    signal::signal_set(signo, handler, safety, ops.callback, ops.has_reactor)
}
/// `swoole_signal_get_handler`.
#[must_use]
pub fn get(signo: i32, ops: &Ops) -> Handler {
    let lookup = signal::signal_lookup(signo);
    if lookup == signal::Lookup::OutOfRange {
        (ops.report)(Report::HandlerOutOfRange(signo));
    }
    lookup.handler()
}
/// `swoole_signal_callback`: runs the handler registered for `signo`.
pub fn callback(signo: i32, ops: &Ops) {
    match signal::signal_lookup(signo) {
        signal::Lookup::OutOfRange => (ops.report)(Report::CallbackOutOfRange(signo)),
        signal::Lookup::Default => (ops.report)(Report::CallbackUnregistered(signo)),
        signal::Lookup::Handler(handler) => handler.call(signo),
        signal::Lookup::Ignored => {}
    }
}
/// The original unsigned listener sum, including async waiters.
#[must_use]
pub fn listener_num() -> u32 {
    corebase::swr_v1_core_signal_listener_num()
        .wrapping_add(corebase::swr_v1_core_signal_async_listener_num())
}
#[cfg(target_os = "linux")]
fn setup(ops: &Ops) -> bool {
    let Some(descriptor) = ops.descriptor else {
        return false;
    };
    if crate::signalfd::signalfd_fd() == -1 {
        use crate::signalfd::CreateError;
        match crate::signalfd::signalfd_create() {
            Ok(fd) => {
                (descriptor.make)(fd);
                corebase::swr_v1_core_set_signal_fd(fd);
            }
            Err(CreateError::Exists) => return false,
            Err(CreateError::Signalfd(error)) => {
                (ops.report)(Report::SignalfdFailed(error));
                return false;
            }
            Err(CreateError::Mask(error)) => {
                (ops.report)(Report::MaskFailed(error));
                return false;
            }
        }
    }
    if !(descriptor.installed)() {
        (descriptor.install)();
    }
    let read = corebase::constants::EventType::SW_EVENT_READ as i32;
    if (descriptor.events)() & read == 0 && !(descriptor.add)(read) {
        return false;
    }
    (descriptor.erase_end)();
    true
}
#[cfg(target_os = "linux")]
fn close(ops: &Ops) {
    let Some(descriptor) = ops.descriptor else {
        return;
    };
    if crate::signalfd::signalfd_fd() == -1 {
        return;
    }
    (descriptor.free)();
    crate::signalfd::signalfd_close();
}
/// `swoole_signal_clear`, on the thread that owns the handlers.
pub fn clear(ops: &Ops) {
    let caps = (ops.capabilities)();
    #[cfg(target_os = "linux")]
    if caps & 1 != 0
        && corebase::swr_v1_core_enable_signalfd()
        && crate::signalfd::signalfd_fd() != -1
    {
        if !crate::signalfd::signalfd_clear() {
            (ops.report)(Report::UnblockFailed(errno()));
        }
        close(ops);
        return;
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    if caps & 2 != 0 && corebase::swr_v1_core_enable_kqueue() {
        let available = (ops.available)();
        let kq = if available { (ops.native_handle)() } else { -1 };
        // The actual kqueue, or -1 when unavailable.
        crate::signal_kqueue::signal_kqueue_clear(kq, available);
        return;
    }
    signal::signal_clear();
}
/// The signalfd's read event: runs the handler for the signal read from `fd`.
#[cfg(target_os = "linux")]
#[must_use]
pub fn read(fd: i32, ops: &Ops) -> i32 {
    use crate::signalfd::Received;
    match crate::signalfd::signalfd_receive(fd) {
        Received::ReadFailed(error) => (ops.report)(Report::ReadFailed(error)),
        Received::OutOfRange(signo) => (ops.report)(Report::UnknownSignal(signo.cast_signed())),
        Received::NoHandler(signo) => (ops.report)(Report::ReadUnregistered(signo.cast_signed())),
        Received::Handler { signo, handler } => handler.call(signo.cast_signed()),
        Received::Nothing => {}
    }
    0
}
/// The reactor is destroyed: this process removes its signalfd wrapper from it.
#[cfg(target_os = "linux")]
pub fn destroyed(ops: &Ops) {
    if crate::signalfd::signalfd_owned()
        && let Some(descriptor) = ops.descriptor
    {
        (descriptor.del)();
    }
}
/// The signal loop's exit condition: one event fewer, with upstream's unsigned wrap.
pub fn exit_condition(event_num: &mut usize) -> bool {
    *event_num = event_num.wrapping_sub(1);
    true
}
