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
    // SAFETY: as the caller promises.
    unsafe { *reactor.cast::<ReactorPrepared>() }
}

/// `int Reactor::trigger_close_event(Event *)`: the default error handler, or `-1`
/// when none is set.
///
/// # Safety
/// `reactor` is a live `Reactor` and `event` an event its handler accepts.
pub unsafe fn trigger_close_event(reactor: *mut c_void, event: *mut c_void) -> c_int {
    // SAFETY: as the caller promises.
    let handlers = unsafe { prepared(reactor) }.handlers;
    // SAFETY: the live reactor's error default slot.
    let slot = unsafe { crate::swr_v1_reactor_handlers_default(handlers, 2) };
    // SAFETY: as above.
    match unsafe { *slot } {
        // SAFETY: as the caller promises.
        Some(handler) => unsafe { handler(reactor, event) },
        None => -1,
    }
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
    // SAFETY: as the caller promises.
    let owner = unsafe { prepared(reactor) }.owner;
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
    // SAFETY: as the caller promises.
    let owner = unsafe { prepared(reactor) }.owner;
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
    // SAFETY: as the caller promises.
    let handlers = unsafe { prepared(reactor) }.handlers;
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
    // SAFETY: as the caller promises.
    let handlers = unsafe { prepared(reactor) }.handlers;
    // SAFETY: the reactor's live handler tables.
    unsafe { &*handlers }.isset(fd_type, event)
}
