// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc (Swoole contributors).
//! event.cc's `swoole_event_*` C API over the thread's `Reactor` (`SwooleTG.reactor`).
//! With `upstream-symbols` those C++ or a test crate calls are defined under their
//! upstream (Itanium) symbols.
//! `swoole_event_init` and `swoole_event_free` are `reactor_state.rs`'s, with the
//! `Reactor`'s constructor and destructor; `swoole_event_defer` is callbacks.rs's.
use crate::handlers::Handler;
use crate::reactor_upstream::{prepared, reactor_change, reactor_interest};
use crate::{ReactorFields, SocketRecord};
use std::ffi::{c_int, c_void};

/// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const EVENT_READ: c_int = 512;
const EVENT_WRITE: c_int = 1024;

/// `SwooleTG.reactor`.
fn current() -> *mut c_void {
    swoole_rs_corebase::thread_reactor()
}

/// A field of the thread's `Reactor`, or `None` without one.
fn field<T>(read: impl FnOnce(&ReactorFields) -> T) -> Option<T> {
    let reactor = current();
    if reactor.is_null() {
        return None;
    }
    // SAFETY: `SwooleTG.reactor` is live until `swoole_event_free` clears it.
    let fields = unsafe { prepared(reactor) }.fields;
    // SAFETY: a live reactor's fields, borrowed for the read alone.
    Some(read(unsafe { &*fields }))
}

/// The `interest` kind `swoole_event_add_or_update` adds for `event`, if it names one.
fn direction(event: c_int) -> Option<u32> {
    match event {
        EVENT_READ => Some(2),
        EVENT_WRITE => Some(3),
        _ => None,
    }
}

/// The thread's `Reactor` while `swoole_event_is_available()`, else null.
#[must_use]
pub fn swr_v1_event_reactor() -> *mut c_void {
    if swoole_event_is_available() {
        current()
    } else {
        std::ptr::null_mut()
    }
}

/// The thread's `Reactor`, available or not (`sw_reactor()`).
#[must_use]
pub fn swr_v1_event_current() -> *mut c_void {
    current()
}

/// `bool swoole_event_is_available()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z25swoole_event_is_availablev")
)]
#[must_use]
pub extern "C" fn swoole_event_is_available() -> bool {
    field(|fields| !fields.destroyed).unwrap_or(false)
}

/// `bool swoole_event_is_running()`.
#[must_use]
pub fn swoole_event_is_running() -> bool {
    field(|fields| fields.running).unwrap_or(false)
}

/// `int swoole_event_add(Socket *socket, int events)`.
///
/// # Safety
/// As `Reactor::add`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z16swoole_event_addPN6swoole7network6SocketEi")
)]
pub unsafe extern "C" fn swoole_event_add(socket: *mut SocketRecord, events: c_int) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { reactor_change(current(), socket, 0, events) }
}

/// `int swoole_event_set(Socket *socket, int events)`.
///
/// # Safety
/// As `Reactor::set`, with a live `SwooleTG.reactor`.
pub unsafe fn swoole_event_set(socket: *mut SocketRecord, events: c_int) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { reactor_change(current(), socket, 1, events) }
}

/// `int swoole_event_del(Socket *socket)`.
///
/// # Safety
/// As `Reactor::del`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z16swoole_event_delPN6swoole7network6SocketE")
)]
pub unsafe extern "C" fn swoole_event_del(socket: *mut SocketRecord) -> c_int {
    // SAFETY: as the caller promises.
    unsafe { reactor_change(current(), socket, 2, 0) }
}

/// `int swoole_event_add_or_update(Socket *socket, int event)`: `add_read_event` or
/// `add_write_event`; upstream asserts the event is one of them.
///
/// # Safety
/// As `Reactor::add_read_event`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z26swoole_event_add_or_updatePN6swoole7network6SocketEi")
)]
pub unsafe extern "C" fn swoole_event_add_or_update(
    socket: *mut SocketRecord,
    event: c_int,
) -> c_int {
    let Some(kind) = direction(event) else {
        debug_assert!(false, "swoole_event_add_or_update: event {event}");
        return -1;
    };
    // SAFETY: as the caller promises.
    unsafe { reactor_interest(current(), socket, kind, 0) }
}

/// `ssize_t swoole_event_write(Socket *socket, const void *data, size_t len)`.
///
/// # Safety
/// As `Reactor::write`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z18swoole_event_writePN6swoole7network6SocketEPKvm")
)]
pub unsafe extern "C-unwind" fn swoole_event_write(
    socket: *mut SocketRecord,
    data: *const c_void,
    len: usize,
) -> isize {
    let reactor = current();
    // The constructor sets it; upstream calls it unchecked.
    let Some(write) = field(|fields| fields.write).flatten() else {
        std::process::abort()
    };
    // SAFETY: as the caller promises.
    unsafe { write(reactor, socket, data, len) }
}

/// `ssize_t swoole_event_writev(Socket *socket, const iovec *iov, size_t iovcnt)`.
///
/// # Safety
/// As `Reactor::writev`, with a live `SwooleTG.reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_event_writevPN6swoole7network6SocketEPK5iovecm")
)]
pub unsafe extern "C-unwind" fn swoole_event_writev(
    socket: *mut SocketRecord,
    iov: *const libc::iovec,
    iovcnt: usize,
) -> isize {
    let reactor = current();
    let Some(writev) = field(|fields| fields.writev).flatten() else {
        std::process::abort()
    };
    // SAFETY: as the caller promises.
    unsafe { writev(reactor, socket, iov, iovcnt) }
}

/// `void swoole_event_set_handler(int fd_type, int event, ReactorHandler handler)`.
///
/// # Safety
/// `SwooleTG.reactor` is live.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z24swoole_event_set_handleriiPFiPN6swoole7ReactorEPNS_5EventEE")
)]
pub unsafe extern "C" fn swoole_event_set_handler(
    fd_type: c_int,
    event: c_int,
    handler: Option<Handler>,
) {
    // SAFETY: as the caller promises.
    unsafe { crate::reactor_upstream::reactor_set_handler(current(), fd_type, event, handler) };
}

/// `bool swoole_event_isset_handler(int fd_type, int event)`.
///
/// # Safety
/// `SwooleTG.reactor` is live.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z26swoole_event_isset_handlerii")
)]
#[must_use]
pub unsafe extern "C" fn swoole_event_isset_handler(fd_type: c_int, event: c_int) -> bool {
    // SAFETY: as the caller promises.
    unsafe { crate::reactor_upstream::reactor_isset_handler(current(), fd_type, event) }
}

/// `Socket *swoole_event_get_socket(int fd)`.
///
/// # Safety
/// `SwooleTG.reactor` is live.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z23swoole_event_get_socketi")
)]
#[must_use]
pub unsafe extern "C" fn swoole_event_get_socket(fd: c_int) -> *mut c_void {
    // SAFETY: as the caller promises.
    let registry = unsafe { prepared(current()) }.registry;
    // SAFETY: the live reactor's registry.
    unsafe { crate::swr_v1_reactor_registry_get(registry, fd) }
}

/// `int swoole_event_wait()`: run the loop unless it may already exit, then free it.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[unsafe(export_name = "_Z17swoole_event_waitv")]
#[must_use]
pub extern "C-unwind" fn swoole_event_wait() -> c_int {
    use crate::callbacks::{reactor_if_exit, reactor_wait};
    let reactor = current();
    let wait_exit = field(|fields| fields.wait_exit).unwrap_or(false);
    // SAFETY: the thread's live reactor; `if_exit` runs only under wait_exit, as upstream's `or`.
    let exit = wait_exit && unsafe { reactor_if_exit(reactor) };
    let retval = if exit {
        0
    } else {
        // SAFETY: as above.
        unsafe { reactor_wait(reactor) }
    };
    // The thread's reactor, which nothing here borrows any more.
    crate::swoole_event_free();
    retval
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_or_update_names_the_read_or_write_interest() {
        // SW_EVENT_READ is 512 and SW_EVENT_WRITE 1024; nothing else is accepted.
        for event in -2048..=2048 {
            let expected = match event {
                512 => Some(2),
                1024 => Some(3),
                _ => None,
            };
            assert_eq!(direction(event), expected, "{event}");
        }
    }

    #[test]
    fn without_a_reactor_the_loop_is_neither_available_nor_running() {
        // No swoole_init: SwooleTG.reactor is null.
        assert!(!swoole_event_is_available());
        assert!(!swoole_event_is_running());
        assert!(swr_v1_event_reactor().is_null());
        assert!(swr_v1_event_current().is_null());
    }
}
