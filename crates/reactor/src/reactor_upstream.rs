// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! `Reactor` members of base.cc that are whole Rust operations. Each receives the
//! `Reactor`, which begins with its prepared Rust state. With `upstream-symbols`
//! they are defined under their upstream (Itanium) symbols.
use crate::handlers::Handler;
use crate::socket_upstream::swr_v1_reactor_log;
use crate::{ReactorPrepared, SocketRecord};
use std::ffi::{c_int, c_void};

/// The prepared state a `Reactor` begins with.
///
/// # Safety
/// `reactor` is a live `Reactor`.
#[must_use]
pub unsafe fn prepared(reactor: *const c_void) -> ReactorPrepared {
    ReactorPrepared::of(reactor)
}

/// `int Reactor::trigger_close_event(Event *)`: the default error handler, or `-1`
/// when none is set.
///
/// # Safety
/// `reactor` is a live `Reactor` and `event` an event its handler accepts.
pub unsafe fn trigger_close_event(reactor: *mut c_void, event: *mut c_void) -> c_int {
    let handlers = ReactorPrepared::of(reactor).handlers;
    // SAFETY: the live reactor's handler tables; a copy of the error default.
    let Some(handler) = (unsafe { (*handlers).defaults[1] }) else {
        return -1;
    };
    // SAFETY: as the caller promises, the live reactor on its thread.
    let reactor = unsafe { crate::Running::new(reactor) };
    // SAFETY: as the caller promises, an event lent for the call.
    let event = unsafe { &mut *event.cast() };
    handler(reactor, event)
}

/// A change's status: `-2` (add could not allocate) is upstream's `bad_alloc`.
fn changed(result: c_int) -> c_int {
    if result == -2 {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<SocketRecord>());
    }
    result
}

/// `int Reactor::change(Socket *socket, uint32_t op, int events) const`.
///
/// # Safety
/// As [`crate::swr_v1_reactor_change`], on a live `Reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7Reactor6changeEPNS_7network6SocketEji")
)]
pub unsafe extern "C" fn reactor_change(
    reactor: *const c_void,
    socket: *mut SocketRecord,
    op: u32,
    events: c_int,
) -> c_int {
    let owner = reactor.cast::<crate::ReactorState>().cast_mut();
    // SAFETY: as the caller promises.
    changed(unsafe { crate::swr_v1_reactor_change(owner, socket, op, events, swr_v1_reactor_log) })
}

/// `int Reactor::interest(Socket *socket, uint32_t kind, int event) const`.
///
/// # Safety
/// As [`crate::swr_v1_reactor_interest`], on a live `Reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7Reactor8interestEPNS_7network6SocketEji")
)]
pub unsafe extern "C" fn reactor_interest(
    reactor: *const c_void,
    socket: *mut SocketRecord,
    kind: u32,
    event: c_int,
) -> c_int {
    let owner = reactor.cast::<crate::ReactorState>().cast_mut();
    // SAFETY: as the caller promises.
    changed(unsafe {
        crate::swr_v1_reactor_interest(owner, socket, kind, event, swr_v1_reactor_log)
    })
}

/// `void Reactor::set_handler(int fd_type, int event, ReactorHandler handler)`:
/// upstream asserts the event names a table.
///
/// # Safety
/// `reactor` is a live `Reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor11set_handlerEiiPFiPS0_PNS_5EventEE")
)]
pub unsafe extern "C" fn reactor_set_handler(
    reactor: *const c_void,
    fd_type: c_int,
    event: c_int,
    handler: Option<Handler>,
) {
    let handlers = ReactorPrepared::of(reactor).handlers;
    // SAFETY: the reactor's live handler tables.
    let set = unsafe { &mut *handlers }.set(fd_type, event, handler);
    debug_assert!(set, "Reactor::set_handler: no table for event {event}");
}

/// `bool Reactor::isset_handler(int fd_type, int event) const`.
///
/// # Safety
/// `reactor` is a live `Reactor`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7Reactor13isset_handlerEii")
)]
#[must_use]
pub unsafe extern "C" fn reactor_isset_handler(
    reactor: *const c_void,
    fd_type: c_int,
    event: c_int,
) -> bool {
    let handlers = ReactorPrepared::of(reactor).handlers;
    // SAFETY: the reactor's live handler tables.
    unsafe { &*handlers }.isset(fd_type, event)
}
