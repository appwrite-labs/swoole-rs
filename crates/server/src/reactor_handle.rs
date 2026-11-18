//! A live `Reactor` handle the server ops act on.
#![cfg_attr(
    not(feature = "upstream-symbols"),
    expect(
        dead_code,
        reason = "the worker ops that use the rest link upstream symbols"
    )
)]
use core::ffi::{c_int, c_void};

use swoole_rs_corebase::reactor::reactor_upstream::{
    change as reactor_change, interest as reactor_interest, reactor_del, set_handlers,
};
use swoole_rs_corebase::reactor::{ChangeOp, Events, Handler, Heap, Interest, Running, Socket};

/// The key a reactor holds its server under: private, so only [`Reactor::hold_server`]
/// stores an object under it.
static SERVER: swoole_rs_corebase::reactor::ReactorObjectKey =
    swoole_rs_corebase::reactor::ReactorObjectKey::new();

/// The server's teardown: this thread's reactor, if it still holds `serv`, holds
/// nothing from now on.
pub(crate) fn release_server(serv: crate::view::Server<'_>) {
    if let Some(reactor) = Reactor::current()
        && reactor
            .server()
            .is_some_and(|held| held.raw() == serv.raw())
    {
        reactor.handle().set_object(core::ptr::null_mut());
    }
}

/// A live `Reactor`, used by the thread that runs it.
#[derive(Clone, Copy)]
pub(crate) struct Reactor(*mut c_void);

impl Reactor {
    /// The reactor a handler runs under.
    pub(crate) fn running(reactor: Running<'_>) -> Self {
        Self(reactor.as_ptr())
    }
    /// The thread's reactor (`sw_reactor()`), which the worker ops run under.
    pub(crate) fn thread() -> Self {
        Self(swoole_rs_corebase::thread_reactor())
    }
    /// The thread's reactor, if it has one.
    pub(crate) fn current() -> Option<Self> {
        let reactor = swoole_rs_corebase::thread_reactor();
        (!reactor.is_null()).then_some(Self(reactor))
    }
    /// `reactor->ptr = serv`: the reactor, which must be this thread's, serves `serv`,
    /// which [`Self::server`] reads back until the reactor is freed, another object
    /// replaces it, or the server's deletion releases it ([`release_server`]).
    pub(crate) fn hold_server(self, serv: crate::view::Server<'_>) {
        if self.0 != swoole_rs_corebase::thread_reactor() {
            swoole_rs_corebase::fatal("a server held by another thread's reactor");
        }
        self.handle().hold(&SERVER, serv.raw());
    }
    /// `reactor->ptr`, while it is the server [`Self::hold_server`] gave it.
    pub(crate) fn server(self) -> Option<crate::view::Server<'static>> {
        let serv = self.handle().object_of(&SERVER)?;
        // SAFETY: only `hold_server` holds an object under the private `SERVER` key, on
        // the holding thread's own reactor. A server is deleted only by its owner's drop
        // (a test server's too), which first releases it from the deleting thread's
        // reactor; any other thread whose reactor holds it is one of the server's own
        // reactor or worker threads, which it joins before that. So a held server is live.
        Some(unsafe { crate::view::Server::from_raw(serv) })
    }
    /// The handle the reactor's own operations take.
    pub(crate) fn handle(self) -> Running<'static> {
        // SAFETY: a live reactor, used by its own thread (the type's invariant), and no
        // reference to its state outlives the call it is passed to.
        unsafe { Running::new(self.0) }
    }
    /// `Reactor::change`'s `op` of `sock` with `events`: whether it applied.
    pub(crate) fn change(self, sock: Heap<'_>, op: ChangeOp, events: Events) -> bool {
        reactor_change(self.handle(), sock, op, events)
    }
    /// `Reactor::interest(sock, kind, 0)`: whether it applied.
    pub(crate) fn interest(self, sock: Heap<'_>, kind: Interest) -> bool {
        reactor_interest(self.handle(), sock, kind, Events::NONE)
    }
    /// `reactor->add(sock, events)`: whether it applied.
    pub(crate) fn add(self, sock: Heap<'_>, events: Events) -> bool {
        self.change(sock, ChangeOp::Add, events)
    }
    /// `reactor->del(sock)`: whether it applied.
    pub(crate) fn del(self, sock: Socket<'_>) -> bool {
        reactor_del(self.handle(), sock)
    }
    /// `reactor->remove_read_event(sock)`.
    pub(crate) fn unread(self, sock: Heap<'_>) {
        let _ = self.interest(sock, Interest::RemoveRead);
    }
    /// `reactor->trigger_close_event(event)`: the default error handler, whether it
    /// succeeded; false with none.
    pub(crate) fn trigger_close(self, event: &mut swoole_rs_corebase::reactor::Event) -> bool {
        swoole_rs_corebase::reactor::reactor_upstream::trigger_close_event(self.handle(), event)
    }
    pub(crate) fn set_handlers(
        self,
        fd_type: c_int,
        read: Option<Handler>,
        write: Option<Handler>,
        error: swoole_rs_corebase::reactor::ErrorHandler,
    ) {
        set_handlers(self.handle(), fd_type, read, write, error);
    }
    /// `reactor->set_default_error_handler(handler)`.
    pub(crate) fn set_error_handler(self, handler: Handler) {
        swoole_rs_corebase::reactor::reactor_upstream::set_default_error_handler(
            self.handle(),
            handler,
        );
    }
}
