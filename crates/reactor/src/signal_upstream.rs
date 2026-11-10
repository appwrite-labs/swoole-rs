// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/signal.cc (Swoole contributors).
//! `swoole_signal_set`, `_callback`, `_get_handler`, `_get_listener_num` and `_clear`
//! under their upstream (Itanium) symbols, over crates/process's signal loop, and the
//! event-loop effects that loop asks for: the signalfd socket, its reactor handler,
//! exit condition and destroy callback, and the warnings. The process patch deletes
//! `src/os/signal.cc`.
use crate::ReactorPrepared;
#[cfg(target_os = "linux")]
use crate::SocketRecord;
use crate::socket_sync::set_errno;
use std::ffi::c_int;
#[cfg(target_os = "linux")]
use std::ffi::c_void;
#[cfg(target_os = "linux")]
use std::ptr;
use swoole_rs_corebase::log;
use swoole_rs_process::signal::swoole_signal_to_str;
use swoole_rs_process::signal_loop::{self, Ops};

/// `SW_FD_SIGNAL`, `SW_EVENT_READ`, `SW_LOG_WARNING` and `SW_ERROR_UNREGISTERED_SIGNAL`.
#[cfg(target_os = "linux")]
const FD_SIGNAL: c_int = 9;
#[cfg(target_os = "linux")]
const EVENT_READ: c_int = 1 << 9;
const LOG_WARNING: c_int = 4;
const UNREGISTERED_SIGNAL: c_int = 721;
/// `Reactor::EXIT_CONDITION_SIGNALFD` and `Reactor::PRIORITY_SIGNAL_CALLBACK`.
#[cfg(target_os = "linux")]
const EXIT_CONDITION_SIGNALFD: c_int = 6;
#[cfg(target_os = "linux")]
const PRIORITY_SIGNAL_CALLBACK: c_int = 3;

type SignalHandler = unsafe extern "C" fn(c_int);

// SAFETY: crates/reactor's.
unsafe extern "C" {
    #[cfg(target_os = "linux")]
    #[link_name = "_Z16swoole_event_addPN6swoole7network6SocketEi"]
    fn swoole_event_add(socket: *mut SocketRecord, events: c_int) -> c_int;
    #[cfg(target_os = "linux")]
    #[link_name = "_Z16swoole_event_delPN6swoole7network6SocketE"]
    fn swoole_event_del(socket: *mut SocketRecord) -> c_int;
    #[cfg(target_os = "linux")]
    #[link_name = "_Z26swoole_event_isset_handlerii"]
    fn swoole_event_isset_handler(fd_type: c_int, event: c_int) -> bool;
    #[cfg(target_os = "linux")]
    #[link_name = "_Z24swoole_event_set_handleriiPFiPN6swoole7ReactorEPNS_5EventEE"]
    fn swoole_event_set_handler(
        fd_type: c_int,
        event: c_int,
        handler: unsafe extern "C" fn(*mut c_void, *mut Event) -> c_int,
    );
}

// SAFETY: crates/tls's `Socket::free`, the same archive; declared, since crates/tls
// depends on this crate.
#[cfg(target_os = "linux")]
unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole7network6Socket4freeEv"]
    fn socket_free(socket: *mut SocketRecord);
}

/// `swoole::Event`'s leading descriptor.
#[cfg(target_os = "linux")]
#[repr(C)]
struct Event {
    fd: c_int,
}

/// The fields of the thread's event loop, available or not (`sw_reactor()`).
fn current() -> Option<ReactorPrepared> {
    let reactor = crate::event_api::swr_v1_event_current();
    // SAFETY: null or a live `Reactor`, which begins with its prepared state.
    unsafe { reactor.cast::<ReactorPrepared>().as_ref() }.copied()
}

fn capabilities() -> u8 {
    u8::from(cfg!(target_os = "linux"))
        | u8::from(cfg!(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))) << 1
}

fn available() -> bool {
    !crate::event_api::swr_v1_event_reactor().is_null()
}

fn has_reactor() -> bool {
    current().is_some()
}

fn native_handle() -> i32 {
    // SAFETY: the reactor's live fields.
    current().map_or(-1, |reactor| unsafe { (*reactor.fields).native_handle })
}

/// # Safety
/// `handler` is a `SignalHandler` the loop registered.
unsafe fn invoke(handler: usize, signo: i32) {
    // SAFETY: the loop only stores `SignalHandler`s, and passes back a registered one.
    let handler: SignalHandler = unsafe { std::mem::transmute(handler) };
    // SAFETY: as above.
    unsafe { handler(signo) };
}

fn report(kind: u8, signo: i32, error: i32) {
    let name = || {
        // SAFETY: a static buffer, NUL-terminated.
        unsafe { std::ffi::CStr::from_ptr(swoole_signal_to_str(signo)) }.to_string_lossy()
    };
    let unregistered = |origin| {
        log::error_log(
            LOG_WARNING,
            UNREGISTERED_SIGNAL,
            origin,
            format_args!("Unable to find callback function for signal {}", name()),
        );
    };
    match kind {
        0 => log::warning(
            c"swoole_signal_callback",
            format_args!("signal[{signo}] numberis invalid"),
        ),
        1 => log::warning(
            c"swoole_signal_get_handler",
            format_args!("signal[{signo}] numberis invalid"),
        ),
        2 => unregistered(c"swoole_signal_callback"),
        3 => log::sys_warning_errno(
            error,
            c"swoole_signalfd_create",
            format_args!("signalfd() failed"),
        ),
        4 => log::sys_warning_errno(
            error,
            c"swoole_signalfd_create",
            format_args!("sigprocmask() failed"),
        ),
        5 => log::sys_warning_errno(
            error,
            c"swoole_signalfd_clear",
            format_args!("sigprocmask(SIG_UNBLOCK) failed"),
        ),
        6 => log::sys_warning_errno(
            error,
            c"swoole_signalfd_event_callback",
            format_args!("read from signalfd failed"),
        ),
        7 => log::warning(
            c"swoole_signalfd_event_callback",
            format_args!("unknown signal[{signo}]"),
        ),
        8 => unregistered(c"swoole_signalfd_event_callback"),
        9 => log::sys_warning_errno(
            error,
            c"swoole_signal_kqueue_set",
            format_args!("kevent set signal[{signo}] error"),
        ),
        _ => {}
    }
    if (3..=6).contains(&kind) || kind == 9 {
        set_errno(error);
    }
}

#[cfg(target_os = "linux")]
mod descriptor {
    use super::{
        EVENT_READ, EXIT_CONDITION_SIGNALFD, Event, FD_SIGNAL, OPS, PRIORITY_SIGNAL_CALLBACK,
        SocketRecord, c_int, c_void, current, ptr, socket_free, swoole_event_add, swoole_event_del,
        swoole_event_isset_handler, swoole_event_set_handler,
    };
    use crate::socket_upstream::make_socket_fd;
    use crate::swr_v1_reactor_deferred_push;
    use swoole_rs_process::signal_loop::DescriptorOps;

    pub(super) fn make(fd: i32) -> *mut c_void {
        make_socket_fd(fd, FD_SIGNAL).cast()
    }

    /// Frees the wrapper but not the descriptor, which the signalfd backend closes.
    /// # Safety
    /// `socket` is the loop's live signalfd wrapper.
    pub(super) unsafe fn free(socket: *mut c_void) {
        let socket = socket.cast::<SocketRecord>();
        // SAFETY: the loop's live signalfd wrapper.
        unsafe { (*socket).fd = -1 };
        // SAFETY: as above; nothing uses it afterwards.
        unsafe { socket_free(socket) };
    }

    /// # Safety
    /// `socket` is the loop's live signalfd wrapper.
    pub(super) unsafe fn events(socket: *mut c_void) -> i32 {
        // SAFETY: the loop's live signalfd wrapper.
        unsafe { (*socket.cast::<SocketRecord>()).events }
    }

    /// # Safety
    /// `socket` is the loop's live signalfd wrapper.
    pub(super) unsafe fn add(socket: *mut c_void, events: i32) -> i32 {
        // SAFETY: a live wrapper on the thread's available loop.
        unsafe { swoole_event_add(socket.cast(), events) }
    }

    /// # Safety
    /// `socket` is the loop's live signalfd wrapper.
    pub(super) unsafe fn del(socket: *mut c_void) {
        // SAFETY: a live wrapper the thread's loop holds.
        unsafe { swoole_event_del(socket.cast()) };
    }

    pub(super) fn installed() -> bool {
        // SAFETY: the thread's available loop.
        unsafe { swoole_event_isset_handler(FD_SIGNAL, EVENT_READ) }
    }

    unsafe extern "C" fn read(_reactor: *mut c_void, event: *mut Event) -> c_int {
        // SAFETY: the reactor lends the live event.
        let fd = unsafe { (*event).fd };
        swoole_rs_process::signal_loop::read(fd, &OPS)
    }

    unsafe extern "C-unwind" fn destroyed(_data: *mut c_void) {
        swoole_rs_process::signal_loop::destroyed(&OPS);
    }

    /// The signalfd's exit condition, which the reactor calls with its event count.
    extern "C-unwind" fn exit_condition(event_num: *mut usize) -> bool {
        // SAFETY: the reactor lends its live, writable event count for the call.
        unsafe { swoole_rs_process::signal_loop::exit_condition(event_num) }
    }

    /// The read handler, the exit condition and the destroy callback.
    pub(super) fn install() {
        // SAFETY: the thread's available loop.
        unsafe { swoole_event_set_handler(FD_SIGNAL, EVENT_READ, read) };
        crate::swr_v1_event_set_exit_condition(EXIT_CONDITION_SIGNALFD, exit_condition);
        let Some(reactor) = current() else { return };
        // SAFETY: the loop's live fields.
        let queue = unsafe { &raw mut (*reactor.fields).destroy_callbacks };
        // SAFETY: the loop's live destroy queue; the call keeps no data.
        let pushed =
            unsafe { swr_v1_reactor_deferred_push(queue, destroyed, ptr::null_mut(), None) };
        if pushed != 0 {
            std::process::abort();
        }
    }

    pub(super) fn erase_end() {
        let Some(reactor) = current() else { return };
        // SAFETY: the thread's live loop, borrowed for this change alone.
        if let Some(owner) = unsafe { reactor.owner.as_mut() } {
            crate::swr_v1_reactor_set_end(owner, PRIORITY_SIGNAL_CALLBACK, None);
        }
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
    invoke,
    report,
};

/// `SignalHandler swoole_signal_set(int signo, SignalHandler handler, bool safety)`.
///
/// # Safety
/// `handler` is null or a function nothing unloads.
#[must_use]
#[unsafe(export_name = "_Z17swoole_signal_setiPFviEb")]
pub unsafe extern "C" fn swoole_signal_set(signo: c_int, handler: usize, safety: bool) -> usize {
    signal_loop::set(signo, handler, safety, &OPS)
}

/// `void swoole_signal_callback(int signo)`: runs the handler registered for `signo`.
///
/// # Safety
/// Called where the handler may run.
#[unsafe(export_name = "_Z22swoole_signal_callbacki")]
pub unsafe extern "C" fn swoole_signal_callback(signo: c_int) {
    signal_loop::callback(signo, &OPS);
}

/// `SignalHandler swoole_signal_get_handler(int signo)`.
#[must_use]
#[unsafe(export_name = "_Z25swoole_signal_get_handleri")]
pub extern "C" fn swoole_signal_get_handler(signo: c_int) -> usize {
    signal_loop::get(signo, &OPS)
}

/// `uint32_t swoole_signal_get_listener_num()`.
#[must_use]
#[unsafe(export_name = "_Z30swoole_signal_get_listener_numv")]
pub extern "C" fn swoole_signal_get_listener_num() -> u32 {
    signal_loop::listener_num()
}

/// `void swoole_signal_clear()`, on the thread that owns the signal handlers.
#[unsafe(export_name = "_Z19swoole_signal_clearv")]
pub extern "C" fn swoole_signal_clear() {
    signal_loop::clear(&OPS);
}
