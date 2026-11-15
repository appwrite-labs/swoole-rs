// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! `Reactor` members of base.cc that are whole Rust operations. Each receives the
//! `Reactor`, which begins with its prepared Rust state. With `upstream-symbols`
//! they are defined under their upstream (Itanium) symbols.
use crate::SocketRecord;
use crate::handlers::Handler;
use crate::socket_upstream::swr_v1_reactor_log;
use std::ffi::{c_int, c_void};

/// `int Reactor::trigger_close_event(Event *)`: the default error handler, or `-1`
/// when none is set.
pub fn trigger_close_event(reactor: crate::Running<'_>, event: &mut crate::Event) -> c_int {
    let Some(handler) = reactor.handlers().fallback(1) else {
        return -1;
    };
    handler(reactor, event)
}

/// `int Reactor::change(Socket *socket, uint32_t op, int events) const`: -1 for a null
/// socket.
///
/// # Safety
/// `reactor` is a live `Reactor` on this thread, which no Rust reference borrows, and
/// `socket` null or a live record distinct from it that stays live while it is
/// registered.
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
    // SAFETY: as the caller promises: live while it is registered, which is all a
    // registered socket's `'static` claims.
    let Some(socket) = (unsafe { crate::Socket::from_ptr(socket) }) else {
        return -1;
    };
    // SAFETY: as the caller promises.
    let reactor = unsafe { crate::Running::new(reactor.cast_mut()) };
    if change(
        reactor,
        socket,
        crate::ChangeOp::from_code(op),
        crate::Events::from_bits(events),
    ) {
        0
    } else {
        -1
    }
}

/// `Reactor::add` or `set` of a socket that is never freed while registered: whether
/// it worked.
/// A registered record's address stays in the registry and with the kernel until its
/// removal, which a `'static` record outlives: it is live until an unsafe free, whose
/// caller answers for every user left, the reactor among them, or its `del`.
#[must_use]
pub fn change(
    reactor: crate::Running<'_>,
    socket: crate::Socket<'static>,
    op: crate::ChangeOp,
    events: crate::Events,
) -> bool {
    let request = crate::change::Request::new(socket, op);
    // The log reaches no reactor.
    reactor.with(|state| crate::change::apply(state, request, events, swr_v1_reactor_log))
}

/// `Reactor::interest` of a socket that is never freed while registered, as [`change`]
/// takes one: whether the change worked, true when nothing changes.
#[must_use]
pub fn interest(
    reactor: crate::Running<'_>,
    socket: crate::Socket<'static>,
    kind: crate::Interest,
    event: crate::Events,
) -> bool {
    crate::change::interest_change(socket, kind, event)
        .is_none_or(|(op, events)| change(reactor, socket, op, events))
}

/// `int Reactor::del(Socket *socket)` of any live socket: a removal leaves the
/// record's address neither in the registry nor with the kernel. Whether it worked.
#[must_use]
pub fn reactor_del(reactor: crate::Running<'_>, socket: crate::Socket<'_>) -> bool {
    let request = crate::change::Request::Del(socket);
    // The log reaches no reactor.
    reactor
        .with(|state| crate::change::apply(state, request, crate::Events::NONE, swr_v1_reactor_log))
}

/// `int Reactor::interest(Socket *socket, uint32_t kind, int event) const`.
///
/// # Safety
/// As [`reactor_change`].
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
    let Some(record) = (unsafe { crate::Socket::from_ptr(socket) }) else {
        return -1;
    };
    let kind = crate::Interest::from_code(kind);
    let Some((op, events)) =
        crate::change::interest_change(record, kind, crate::Events::from_bits(event))
    else {
        return 0;
    };
    // SAFETY: as the caller promises.
    unsafe { reactor_change(reactor, socket, op.code(), events.bits()) }
}

/// `void Reactor::set_handler(int fd_type, int event, ReactorHandler handler)`:
/// upstream asserts the event names a table.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole7Reactor11set_handlerEiiPFiPS0_PNS_5EventEE")
)]
pub extern "C" fn reactor_set_handler(
    reactor: crate::Running<'_>,
    fd_type: c_int,
    event: c_int,
    handler: Option<Handler>,
) {
    let set = reactor.handlers().set(fd_type, event, handler);
    debug_assert!(set, "Reactor::set_handler: no table for event {event}");
}

/// `ReactorHandler Reactor::get_handler(FdType fd_type, EventType event) const`: the
/// registered handler, else the write or error default; none for an `event` that is
/// not `SW_EVENT_READ`, `SW_EVENT_WRITE` or `SW_EVENT_ERROR`, where upstream aborts.
#[must_use]
pub fn reactor_get_handler(
    reactor: crate::Running<'_>,
    fd_type: c_int,
    event: c_int,
) -> Option<Handler> {
    reactor.handlers().of_event(fd_type, event)
}

/// `Reactor::default_write_handler`, which `swoole_event_init` sets.
#[must_use]
pub fn reactor_default_write_handler(reactor: crate::Running<'_>) -> Option<Handler> {
    reactor.handlers().fallback(0)
}

/// `bool Reactor::isset_handler(int fd_type, int event) const`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole7Reactor13isset_handlerEii")
)]
#[must_use]
pub extern "C" fn reactor_isset_handler(
    reactor: crate::Running<'_>,
    fd_type: c_int,
    event: c_int,
) -> bool {
    reactor.handlers().isset(fd_type, event)
}
