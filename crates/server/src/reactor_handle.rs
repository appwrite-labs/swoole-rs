//! A live `Reactor` handle the server ops act on.
#![cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(
        dead_code,
        reason = "the worker ops that use the rest link upstream symbols"
    )
)]
use core::ffi::{c_int, c_void};

use swoole_rs_reactor::reactor_upstream::{reactor_change, reactor_interest, reactor_set_handler};
use swoole_rs_reactor::{Handler, ReactorPrepared, Running, Socket};

/// `Reactor::change`'s add and del, `Reactor::interest`'s `remove_read_event`.
const ADD: u32 = 0;
const DEL: u32 = 2;
const REMOVE_READ: u32 = 4;
/// `SW_EVENT_READ`.
pub(crate) const EVENT_READ: c_int = 1 << 9;

/// A live `Reactor`, used by the thread that runs it.
#[derive(Clone, Copy)]
pub(crate) struct Reactor(*mut c_void);

impl Reactor {
    /// # Safety
    /// `reactor` is a live `Reactor` while the handle is used, on its own thread.
    pub(crate) unsafe fn of(reactor: *mut c_void) -> Self {
        Self(reactor)
    }
    /// The reactor a handler runs under.
    pub(crate) fn running(reactor: Running<'_>) -> Self {
        Self(reactor.as_ptr())
    }
    /// The thread's reactor (`sw_reactor()`), which the worker ops run under.
    pub(crate) fn thread() -> Self {
        Self(swoole_rs_corebase::thread_reactor())
    }
    pub(crate) fn ptr(self) -> *mut c_void {
        self.0
    }
    /// The handle the reactor's own operations take.
    pub(crate) fn handle(self) -> Running<'static> {
        // SAFETY: a live reactor, used by its own thread (the type's invariant), and no
        // reference to its state outlives the call it is passed to.
        unsafe { Running::new(self.0) }
    }
    /// `Reactor::change`'s `op` (0 add, 1 set, 2 del) of `sock` with `events`.
    pub(crate) fn change(self, sock: Socket<'_>, op: u32, events: c_int) -> c_int {
        // SAFETY: a live reactor and socket.
        unsafe { reactor_change(self.0, sock.as_ptr(), op, events) }
    }
    /// `Reactor::interest`'s `kind` (2 add read, 3 add write, 4 remove read) of `sock`.
    pub(crate) fn interest(self, sock: Socket<'_>, kind: u32, events: c_int) -> c_int {
        // SAFETY: a live reactor and socket.
        unsafe { reactor_interest(self.0, sock.as_ptr(), kind, events) }
    }
    /// `reactor->add(sock, events)`.
    pub(crate) fn add(self, sock: Socket<'_>, events: c_int) -> c_int {
        self.change(sock, ADD, events)
    }
    /// `reactor->del(sock)`.
    pub(crate) fn del(self, sock: Socket<'_>) -> c_int {
        self.change(sock, DEL, 0)
    }
    /// `reactor->remove_read_event(sock)`.
    pub(crate) fn unread(self, sock: Socket<'_>) {
        let _ = self.interest(sock, REMOVE_READ, 0);
    }
    /// The reactor's own `close` of `sock`, when it has one.
    pub(crate) fn close(self, sock: Socket<'_>) -> c_int {
        self.handle().close().map_or(0, |close| {
            // SAFETY: the reactor's close on one of its live sockets.
            unsafe { close(self.0, sock.as_ptr().cast()) }
        })
    }
    /// `reactor->trigger_close_event(event)`: the default error handler, or `-1` with none.
    pub(crate) fn trigger_close(self, event: &mut swoole_rs_reactor::Event) -> c_int {
        swoole_rs_reactor::reactor_upstream::trigger_close_event(self.handle(), event)
    }
    pub(crate) fn set_handler(self, fd_type: c_int, event: c_int, handler: Handler) {
        reactor_set_handler(self.handle(), fd_type, event, Some(handler));
    }
    /// `reactor->set_default_error_handler(handler)`.
    pub(crate) fn set_error_handler(self, handler: Handler) {
        // A live reactor.
        let handlers = ReactorPrepared::of(self.0).handlers;
        // SAFETY: its live handler tables, which nothing else borrows meanwhile.
        let handlers = unsafe { &mut *handlers };
        swoole_rs_reactor::swr_v1_reactor_handlers_set_default(handlers, 2, Some(handler));
    }
}
