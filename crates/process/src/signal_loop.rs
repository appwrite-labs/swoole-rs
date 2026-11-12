// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/signal.cc (Swoole contributors).
//! Signal backend routing and signalfd reactor-wrapper custody. The existing
//! backends own handlers/masks/descriptors; SDK leaves lend reactor/socket effects.
use crate::signal;
use swoole_rs_corebase as corebase;

/// The event loop's side of the signal backends (crates/reactor).
pub struct Ops {
    /// Bit 0: signalfd; bit 1: kqueue.
    pub capabilities: fn() -> u8,
    pub available: fn() -> bool,
    pub has_reactor: fn() -> bool,
    /// The available reactor's kqueue.
    pub native_handle: fn() -> i32,
    pub callback: signal::Callback,
    /// The signalfd socket's reactor effects (Linux).
    pub descriptor: Option<&'static DescriptorOps>,
    /// Run a handler `set` registered.
    pub invoke: unsafe fn(usize, i32),
    pub report: fn(u8, i32, i32),
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
    /// Add the wrapper to the thread's loop for `events`.
    pub add: fn(i32) -> i32,
    /// Remove the wrapper, if any, from the loop.
    pub del: fn(),
    pub installed: fn() -> bool,
    pub install: fn(),
    pub erase_end: fn(),
}
fn report(ops: &Ops, kind: u8, signo: i32, error: i32) {
    (ops.report)(kind, signo, error);
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
pub fn set(signo: i32, handler: usize, safety: bool, ops: &Ops) -> usize {
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
            report(ops, 9, signo, errno());
        }
        return previous;
    }
    signal::signal_set(signo, handler, safety, ops.callback, ops.has_reactor)
}
/// `swoole_signal_get_handler`.
#[must_use]
pub fn get(signo: i32, ops: &Ops) -> usize {
    let lookup = signal::signal_lookup(signo);
    if lookup.status == 1 {
        report(ops, 1, signo, 0);
    }
    lookup.handler
}
/// `swoole_signal_callback`: runs the handler registered for `signo`.
pub fn callback(signo: i32, ops: &Ops) {
    let lookup = signal::signal_lookup(signo);
    match lookup.status {
        1 => report(ops, 0, signo, 0),
        2 => report(ops, 2, signo, 0),
        // SAFETY: a handler `set` registered.
        0 => unsafe { (ops.invoke)(lookup.handler, signo) },
        _ => {}
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
        let created = crate::signalfd::signalfd_create();
        match created.status {
            0 => {
                (descriptor.make)(created.fd);
                corebase::swr_v1_core_set_signal_fd(created.fd);
            }
            1 => return false,
            status => {
                report(ops, if status == 2 { 3 } else { 4 }, 0, created.error);
                return false;
            }
        }
    }
    if !(descriptor.installed)() {
        (descriptor.install)();
    }
    let read = corebase::constants::EventType::SW_EVENT_READ as i32;
    if (descriptor.events)() & read == 0 && (descriptor.add)(read) < 0 {
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
            report(ops, 5, 0, errno());
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
    let received = crate::signalfd::signalfd_receive(fd);
    match received.status {
        1 => report(ops, 6, 0, received.error),
        2 => report(ops, 7, received.signo.cast_signed(), 0),
        4 => report(ops, 8, received.signo.cast_signed(), 0),
        // SAFETY: a handler `set` registered.
        0 => unsafe { (ops.invoke)(received.handler, received.signo.cast_signed()) },
        _ => {}
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
