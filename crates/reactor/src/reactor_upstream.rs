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

/// `Reactor::add` (op 0) or `set` (1) of a socket that is never freed while registered.
/// A registered record's address stays in the registry and with the kernel until its
/// removal, which a `'static` record outlives: it is live until an unsafe free, whose
/// caller answers for every user left, the reactor among them. Any other op is `del`.
#[must_use]
pub fn change(
    reactor: crate::Running<'_>,
    socket: crate::Socket<'static>,
    op: u32,
    events: c_int,
) -> c_int {
    // SAFETY: the handle's live state, which no reference borrows (its contract), and
    // a record that is live for as long as it stays registered; the log reads neither.
    changed(unsafe {
        crate::swr_v1_reactor_change(
            reactor.state(),
            socket.as_ptr(),
            op,
            events,
            swr_v1_reactor_log,
        )
    })
}

/// `Reactor::interest` (`add_event` 0, `del_event` 1, `add_read_event` 2,
/// `add_write_event` 3, `remove_read_event` 4, `remove_write_event` 5) of a socket that
/// is never freed while registered, as [`change`] takes one: the change's result, or 0
/// when nothing changes.
#[must_use]
pub fn interest(
    reactor: crate::Running<'_>,
    socket: crate::Socket<'static>,
    kind: u32,
    event: c_int,
) -> c_int {
    crate::change::interest_change(socket, kind, event)
        .map_or(0, |(op, events)| change(reactor, socket, op, events))
}

/// `int Reactor::del(Socket *socket)` of any live socket: a removal leaves the
/// record's address neither in the registry nor with the kernel.
pub fn reactor_del(reactor: crate::Running<'_>, socket: crate::Socket<'_>) -> c_int {
    // SAFETY: the handle's live state, which no reference borrows (its contract), and
    // a live record; del registers nothing, and the log reads neither.
    unsafe {
        crate::swr_v1_reactor_change(reactor.state(), socket.as_ptr(), 2, 0, swr_v1_reactor_log)
    }
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
