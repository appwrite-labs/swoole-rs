// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/reactor/base.cc (Swoole contributors).
//! `Reactor` members of base.cc that are whole Rust operations. Each receives the
//! `Reactor`, which begins with its prepared Rust state.
use crate::reactor::handlers::Handler;
use crate::reactor::socket_upstream::swr_v1_reactor_log;
use std::ffi::c_int;

/// `int Reactor::trigger_close_event(Event *)`: the socket type's error handler, whether
/// it succeeded; false when none is set.
pub fn trigger_close_event(
    reactor: crate::reactor::Running<'_>,
    event: &mut crate::reactor::Event,
) -> bool {
    let Some(socket) = event.socket() else {
        return false;
    };
    let fd_type = socket.read(|record| record.fd_type);
    // The lookup's borrow ends before the handler runs.
    reactor
        .handlers(|handlers| handlers.of_event(fd_type, crate::reactor::Events::ERROR))
        .is_some_and(|handler| handler(reactor, event))
}

/// `Reactor::add` or `set` of a heap socket: whether it worked.
/// A registered record's address stays in the registry and with the kernel until its
/// removal: its owner's free deletes it first (`OwnedSocket::into_box`), and a record
/// no owner frees outlives it, as [`crate::reactor::Heap`] states.
#[must_use]
pub fn change(
    reactor: crate::reactor::Running<'_>,
    socket: crate::reactor::Heap<'_>,
    op: crate::reactor::ChangeOp,
    events: crate::reactor::Events,
) -> bool {
    let request = crate::reactor::change::Request::new(socket, op);
    // The log reaches no reactor.
    reactor.with(|state| crate::reactor::change::apply(state, request, events, swr_v1_reactor_log))
}

/// `Reactor::interest` of a heap socket, as [`change`] takes one: whether the change
/// worked, true when nothing changes.
#[must_use]
pub fn interest(
    reactor: crate::reactor::Running<'_>,
    socket: crate::reactor::Heap<'_>,
    kind: crate::reactor::Interest,
    event: crate::reactor::Events,
) -> bool {
    socket
        .edit(|record| record.interest_change(kind, event))
        .is_none_or(|(op, events)| change(reactor, socket, op, events))
}

/// `int Reactor::del(Socket *socket)` of any live socket: a removal leaves the
/// record's address neither in the registry nor with the kernel. Whether it worked.
#[must_use]
pub fn reactor_del(
    reactor: crate::reactor::Running<'_>,
    socket: crate::reactor::Socket<'_>,
) -> bool {
    let request = crate::reactor::change::Request::Del(socket);
    // The log reaches no reactor.
    reactor.with(|state| {
        crate::reactor::change::apply(
            state,
            request,
            crate::reactor::Events::NONE,
            swr_v1_reactor_log,
        )
    })
}

/// Replace the read, write and error registrations for a descriptor type.
pub fn set_handlers(
    reactor: crate::reactor::Running<'_>,
    fd_type: c_int,
    read: Option<Handler>,
    write: Option<Handler>,
    error: crate::reactor::ErrorHandler,
) {
    let set = reactor.handlers(|handlers| handlers.register(fd_type, read, write, error));
    debug_assert!(set, "invalid descriptor type {fd_type}");
}

/// `ReactorHandler Reactor::get_handler(FdType fd_type, EventType event) const`: the
/// registered handler, else the write or error default; none for an `event` that is
/// not `SW_EVENT_READ`, `SW_EVENT_WRITE` or `SW_EVENT_ERROR`, where upstream aborts.
#[must_use]
pub fn reactor_get_handler(
    reactor: crate::reactor::Running<'_>,
    fd_type: c_int,
    event: crate::reactor::Events,
) -> Option<Handler> {
    reactor.handlers(|handlers| handlers.of_event(fd_type, event))
}

/// `reactor->set_default_error_handler(handler)`.
pub fn set_default_error_handler(reactor: crate::reactor::Running<'_>, handler: Handler) {
    reactor.handlers(|handlers| {
        crate::reactor::swr_v1_reactor_handlers_set_default(handlers, 2, Some(handler));
    });
}

/// `Reactor::default_write_handler`, which `swoole_event_init` sets.
#[must_use]
pub fn reactor_default_write_handler(reactor: crate::reactor::Running<'_>) -> Option<Handler> {
    reactor.handlers(|handlers| handlers.fallback(0))
}

/// Whether the reactor has a handler for this descriptor type and event.
#[must_use]
pub fn isset_handler(
    reactor: crate::reactor::Running<'_>,
    fd_type: c_int,
    event: crate::reactor::Events,
) -> bool {
    reactor.handlers(|handlers| handlers.isset(fd_type, event))
}
