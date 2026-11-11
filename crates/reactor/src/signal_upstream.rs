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
use swoole_rs_corebase::log;
use swoole_rs_process::signal::signal_name;
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
}

// SAFETY: crates/tls's `Socket::free`, the same archive; declared, since crates/tls
// depends on this crate.
#[cfg(target_os = "linux")]
unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole7network6Socket4freeEv"]
    fn socket_free(socket: *mut SocketRecord);
}

/// The fields of the thread's event loop, available or not (`sw_reactor()`).
fn current() -> Option<ReactorPrepared> {
    crate::event_api::current_prepared()
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
    crate::event_api::field(|fields| fields.native_handle).unwrap_or(-1)
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
    let signal = signal_name(signo);
    let name = || signal.as_cstr().to_string_lossy();
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
        EVENT_READ, EXIT_CONDITION_SIGNALFD, FD_SIGNAL, OPS, PRIORITY_SIGNAL_CALLBACK, c_int,
        c_void, socket_free, swoole_event_add, swoole_event_del,
    };
    use crate::event_api::{FdHandler, swoole_event_isset_handler, swoole_event_set_fd_handler};
    use crate::socket_upstream::make_socket_fd;
    use crate::{Socket, SocketRecord};
    use core::ptr;
    use core::sync::atomic::{AtomicPtr, Ordering::Relaxed};
    use swoole_rs_process::signal_loop::DescriptorOps;

    /// The signalfd's socket wrapper: null, or the live one `make` created and
    /// `free` has not yet released.
    static SOCKET: AtomicPtr<SocketRecord> = AtomicPtr::new(ptr::null_mut());

    pub(super) fn make(fd: i32) {
        SOCKET.store(make_socket_fd(fd, FD_SIGNAL), Relaxed);
    }

    /// Frees the wrapper but not the descriptor, which the signalfd backend closes.
    pub(super) fn free() {
        // SAFETY: SOCKET's wrapper, which nothing uses once taken out of it.
        let Some(socket) = (unsafe { Socket::from_ptr(SOCKET.swap(ptr::null_mut(), Relaxed)) })
        else {
            return;
        };
        socket.edit(|record| record.fd = -1);
        // SAFETY: as above.
        unsafe { socket_free(socket.as_ptr()) };
    }

    pub(super) fn events() -> i32 {
        // SAFETY: SOCKET's wrapper.
        unsafe { Socket::from_ptr(SOCKET.load(Relaxed)) }
            .map_or(0, |socket| socket.read(|record| record.events))
    }

    pub(super) fn add(events: i32) -> i32 {
        // SAFETY: SOCKET's wrapper, on the thread's available loop.
        unsafe { swoole_event_add(SOCKET.load(Relaxed), events) }
    }

    pub(super) fn del() {
        let socket = SOCKET.load(Relaxed);
        if !socket.is_null() {
            // SAFETY: SOCKET's wrapper, which the thread's loop holds.
            unsafe { swoole_event_del(socket) };
        }
    }

    pub(super) fn installed() -> bool {
        swoole_event_isset_handler(FD_SIGNAL, EVENT_READ)
    }

    /// The signalfd's read handler.
    struct Read;

    impl FdHandler for Read {
        fn handle(fd: c_int) -> c_int {
            swoole_rs_process::signal_loop::read(fd, &OPS)
        }
    }

    extern "C-unwind" fn destroyed(_data: *mut c_void) {
        swoole_rs_process::signal_loop::destroyed(&OPS);
    }

    /// The signalfd's exit condition, which the reactor calls with its event count.
    extern "C-unwind" fn exit_condition(event_num: &mut usize) -> bool {
        swoole_rs_process::signal_loop::exit_condition(event_num)
    }

    /// The read handler, the exit condition and the destroy callback.
    pub(super) fn install() {
        swoole_event_set_fd_handler::<Read>(FD_SIGNAL, EVENT_READ);
        crate::swr_v1_event_set_exit_condition(EXIT_CONDITION_SIGNALFD, exit_condition);
        crate::event_api::on_destroy(destroyed);
    }

    pub(super) fn erase_end() {
        crate::swr_v1_event_erase_end_callback(PRIORITY_SIGNAL_CALLBACK);
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

/// A signal's handler as the signal loop holds it: none, `SIG_IGN`, or a function
/// to call with the signal's number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct Handler(usize);

impl Handler {
    pub const NONE: Self = Self(0);
    pub const IGNORE: Self = Self(libc::SIG_IGN);

    #[must_use]
    pub fn new(handler: extern "C" fn(c_int)) -> Self {
        Self(handler as usize)
    }

    /// # Safety
    /// `handler` is 0, `SIG_IGN`, or a `SignalHandler` that is sound to call with
    /// any signal number and that nothing unloads.
    #[must_use]
    pub const unsafe fn from_raw(handler: usize) -> Self {
        Self(handler)
    }

    #[must_use]
    pub const fn raw(self) -> usize {
        self.0
    }
}

/// `swoole_signal_set(signo, handler, safety)`: the handler it replaced.
#[must_use]
pub fn set(signo: c_int, handler: Handler, safety: bool) -> Handler {
    Handler(signal_loop::set(signo, handler.0, safety, &OPS))
}

/// `swoole_signal_get_handler(signo)`.
#[must_use]
pub fn get(signo: c_int) -> Handler {
    Handler(signal_loop::get(signo, &OPS))
}

/// `swoole_signal_clear()`.
pub fn clear() {
    signal_loop::clear(&OPS);
}

/// `SignalHandler swoole_signal_set(int signo, SignalHandler handler, bool safety)`.
///
/// # Safety
/// `handler` is as [`Handler::from_raw`] requires.
#[must_use]
#[unsafe(export_name = "_Z17swoole_signal_setiPFviEb")]
pub unsafe extern "C" fn swoole_signal_set(signo: c_int, handler: usize, safety: bool) -> usize {
    // SAFETY: as the caller promises.
    set(signo, unsafe { Handler::from_raw(handler) }, safety).0
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
    get(signo).0
}

/// `uint32_t swoole_signal_get_listener_num()`.
#[must_use]
#[unsafe(export_name = "_Z30swoole_signal_get_listener_numv")]
pub extern "C" fn swoole_signal_get_listener_num() -> u32 {
    signal_loop::listener_num()
}

/// `void swoole_signal_clear()`.
#[unsafe(export_name = "_Z19swoole_signal_clearv")]
pub extern "C" fn swoole_signal_clear() {
    clear();
}
