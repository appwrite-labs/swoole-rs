// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/signal.cc (Swoole contributors).
//! Signal backend routing and signalfd reactor-wrapper custody. The existing
//! backends own handlers/masks/descriptors; SDK leaves lend reactor/socket effects.
use crate::signal;
use std::ffi::c_void;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use swoole_rs_corebase as corebase;

#[repr(C)]
pub(crate) struct Ops {
    capabilities: unsafe extern "C" fn() -> u8,
    available: unsafe extern "C" fn() -> bool,
    has_reactor: unsafe extern "C" fn() -> bool,
    native_handle: unsafe extern "C" fn() -> i32,
    callback: signal::Callback,
    descriptor: *const DescriptorOps,
    invoke: unsafe extern "C" fn(usize, i32),
    report: unsafe extern "C" fn(u8, i32, i32),
}
#[repr(C)]
pub(crate) struct DescriptorOps {
    make: unsafe extern "C" fn(i32) -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void),
    events: unsafe extern "C" fn(*mut c_void) -> i32,
    add: unsafe extern "C" fn(*mut c_void, i32) -> i32,
    del: unsafe extern "C" fn(*mut c_void),
    installed: unsafe extern "C" fn() -> bool,
    install: unsafe extern "C" fn(),
    erase_end: unsafe extern "C" fn(),
}
#[cfg(target_os = "linux")]
static SOCKET: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_os = "linux")]
fn descriptor(ops: &Ops) -> &'static DescriptorOps {
    // SAFETY: only the configured signalfd branch calls this; its static native
    // table supplies a descriptor adapter, absent backends supply null.
    unsafe { &*ops.descriptor }
}
unsafe fn native(ops: *const Ops) -> &'static Ops {
    // SAFETY: every facade lends the same immutable process-lifetime SDK table.
    unsafe { &*ops }
}
fn report(ops: &Ops, kind: u8, signo: i32, error: i32) {
    // SAFETY: literal SDK logging observes only scalar arguments.
    unsafe { (ops.report)(kind, signo, error) };
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

/// # Safety
/// `ops` is the static SDK table; native callbacks preserve the owner's thread/context.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_signal_loop_set(
    signo: i32,
    handler: usize,
    safety: bool,
    ops: *const Ops,
) -> usize {
    // SAFETY: the native facade provides its static table.
    let ops = unsafe { native(ops) };
    // SAFETY: capability and availability getters run no callbacks.
    let caps = unsafe { (ops.capabilities)() };
    // SAFETY: the current event-loop availability is a primitive SDK fact.
    let available = unsafe { (ops.available)() };
    #[cfg(target_os = "linux")]
    if caps & 1 != 0 && corebase::swr_v1_core_enable_signalfd() && available {
        let previous = crate::signalfd::swr_v1_signalfd_set(signo, handler);
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
        // SAFETY: the selected backend's live native kqueue handle is borrowed.
        let kq = unsafe { (ops.native_handle)() };
        // SAFETY: a live kqueue and writable result flag; no Rust borrow spans callbacks.
        let previous = unsafe {
            crate::signal_kqueue::swr_v1_signal_kqueue_set(kq, signo, handler, &raw mut failed)
        };
        if failed {
            report(ops, 9, signo, errno());
        }
        return previous;
    }
    signal::swr_v1_signal_set(signo, handler, safety, ops.callback, ops.has_reactor)
}
/// # Safety
/// The facade lends the static SDK table; lookup itself invokes no handler.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_signal_loop_get(signo: i32, ops: *const Ops) -> usize {
    let lookup = signal::swr_v1_signal_lookup(signo);
    if lookup.status == 1 {
        // SAFETY: the facade provides a static SDK logging table.
        report(unsafe { native(ops) }, 1, signo, 0);
    }
    lookup.handler
}
/// # Safety
/// The configured handler is invoked only in the same contexts as the native facade.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_signal_loop_callback(signo: i32, ops: *const Ops) {
    // SAFETY: the facade provides its static SDK table.
    let ops = unsafe { native(ops) };
    let lookup = signal::swr_v1_signal_lookup(signo);
    match lookup.status {
        1 => report(ops, 0, signo, 0),
        2 => report(ops, 2, signo, 0),
        0 => {
            // SAFETY: the registered handler is copied before invocation; no owner/borrow spans reentry.
            unsafe { (ops.invoke)(lookup.handler, signo) };
        }
        _ => {}
    }
}
/// The original unsigned listener sum, including async waiters.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_v1_signal_loop_listener_num() -> u32 {
    corebase::swr_v1_core_signal_listener_num()
        .wrapping_add(corebase::swr_v1_core_signal_async_listener_num())
}
#[cfg(target_os = "linux")]
fn setup(ops: &Ops) -> bool {
    let descriptor = descriptor(ops);
    if crate::signalfd::swr_v1_signalfd_fd() == -1 {
        let created = crate::signalfd::swr_v1_signalfd_create();
        match created.status {
            0 => {
                // SAFETY: the new descriptor is lent to a native wrapper, not transferred to it.
                let socket = unsafe { (descriptor.make)(created.fd) };
                SOCKET.store(socket as usize, Relaxed);
                corebase::swr_v1_core_set_signal_fd(created.fd);
            }
            1 => return false,
            status => {
                report(ops, if status == 2 { 3 } else { 4 }, 0, created.error);
                return false;
            }
        }
    }
    // SAFETY: a current reactor's handler-presence query runs no callbacks.
    if !unsafe { (descriptor.installed)() } {
        // SAFETY: install the native SDK handler/exit/destruction trampolines once.
        unsafe { (descriptor.install)() };
    }
    let socket = SOCKET.load(Relaxed) as *mut c_void;
    // SAFETY: an available descriptor has the live wrapper created above.
    let events = unsafe { (descriptor.events)(socket) };
    let read = corebase::constants::EventType::SW_EVENT_READ as i32;
    // SAFETY: the reactor borrows this live wrapper, with the requested canonical read mask.
    if events & read == 0 && unsafe { (descriptor.add)(socket, read) } < 0 {
        return false;
    }
    // SAFETY: the native effect removes the reactor's signal-dispatch end callback.
    unsafe { (descriptor.erase_end)() };
    true
}
#[cfg(target_os = "linux")]
fn close(ops: &Ops) {
    let descriptor = descriptor(ops);
    if crate::signalfd::swr_v1_signalfd_fd() == -1 {
        return;
    }
    let socket = SOCKET.load(Relaxed) as *mut c_void;
    // SAFETY: detach fd then release only the wrapper, preserving the backend’s descriptor custody.
    unsafe { (descriptor.free)(socket) };
    SOCKET.store(0, Relaxed);
    crate::signalfd::swr_v1_signalfd_close();
}
/// # Safety
/// The native facade lends its static SDK table on the signal owner's thread.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_signal_loop_clear(ops: *const Ops) {
    // SAFETY: the native facade provides its static table.
    let ops = unsafe { native(ops) };
    // SAFETY: configured SDK capabilities are immutable metadata.
    let caps = unsafe { (ops.capabilities)() };
    #[cfg(target_os = "linux")]
    if caps & 1 != 0
        && corebase::swr_v1_core_enable_signalfd()
        && crate::signalfd::swr_v1_signalfd_fd() != -1
    {
        if !crate::signalfd::swr_v1_signalfd_clear() {
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
        // SAFETY: the SDK availability getter has no callbacks.
        let available = unsafe { (ops.available)() };
        // SAFETY: only an available reactor's handle is inspected.
        let kq = if available {
            // SAFETY: an available reactor owns this live native kqueue handle.
            unsafe { (ops.native_handle)() }
        } else {
            -1
        };
        // SAFETY: actual kqueue or -1 when unavailable, as the backend contract requires.
        unsafe { crate::signal_kqueue::swr_v1_signal_kqueue_clear(kq, available) };
        return;
    }
    signal::swr_v1_signal_clear();
}
#[cfg(target_os = "linux")]
/// # Safety
/// A signalfd event lends its live fd and the process-lifetime operations table.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_signal_loop_read(fd: i32, ops: *const Ops) -> i32 {
    // SAFETY: the native trampoline lends its static table.
    let ops = unsafe { native(ops) };
    #[cfg(target_os = "linux")]
    {
        let received = crate::signalfd::swr_v1_signalfd_receive(fd);
        match received.status {
            1 => report(ops, 6, 0, received.error),
            2 => report(ops, 7, received.signo.cast_signed(), 0),
            4 => report(ops, 8, received.signo.cast_signed(), 0),
            0 => {
                // SAFETY: copied registered handler/number; no Rust owner or borrow spans invocation.
                unsafe { (ops.invoke)(received.handler, received.signo.cast_signed()) };
            }
            _ => {}
        }
    }
    0
}
#[cfg(target_os = "linux")]
/// # Safety
/// Called by native reactor destruction with its static table on the owner thread.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_signal_loop_destroyed(ops: *const Ops) {
    #[cfg(target_os = "linux")]
    {
        let socket = SOCKET.load(Relaxed) as *mut c_void;
        if !socket.is_null() && crate::signalfd::swr_v1_signalfd_owned() {
            // SAFETY: the native destruction callback lends its static table.
            let ops = unsafe { native(ops) };
            // SAFETY: only this process removes the borrowed wrapper from its reactor.
            unsafe { (descriptor(ops).del)(socket) };
        }
    }
}
/// # Safety
/// A native exit-condition callback lends its writable event count for this call.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn swr_v1_signal_loop_exit(event_num: *mut usize) -> bool {
    // SAFETY: this callback receives the reactor's live writable counter.
    let count = unsafe { event_num.read() };
    // SAFETY: preserve native unsigned decrement, including wrap, before returning true.
    unsafe { event_num.write(count.wrapping_sub(1)) };
    true
}
