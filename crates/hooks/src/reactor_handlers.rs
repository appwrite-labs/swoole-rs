//! `System::init_reactor`'s readiness handlers for coroutine sockets
//! (`SW_FD_CO_SOCKET`), hooked polls (`SW_FD_CO_POLL`) and event waiters
//! (`SW_FD_CO_EVENT`), registered through `set`; a ready waiter's coroutine resumes
//! through [`wake`], deferred on the thread's reactor.
use crate::socket_members;
use crate::wait_event::{Parked, event_ready};
use core::ffi::{c_int, c_void};
use swoole_rs_corebase::constants::FdType;
use swoole_rs_reactor::{Event, Events, Handler, Running};

/// The fired socket's object and fd.
fn fired(event: &Event) -> (*mut c_void, c_int) {
    let object = event.socket().map_or(core::ptr::null_mut(), |socket| {
        socket.read(|record| record.object)
    });
    (object, event.fd())
}

extern "C" fn poll_ready<const KIND: i32>(_: Running<'_>, event: &mut Event) -> c_int {
    let (task, fd) = fired(event);
    // SAFETY: a `SW_FD_CO_POLL` socket, whose object is its parked poll task.
    let task = unsafe { &*task.cast::<crate::poll::Task<'_>>() };
    crate::poll::ready(task, fd, Events::from_bits(KIND));
    0
}

extern "C" fn waiter_ready<const KIND: i32>(_: Running<'_>, event: &mut Event) -> c_int {
    let (waiter, _) = fired(event);
    // SAFETY: a `SW_FD_CO_EVENT` socket, whose object is its parked waiter.
    let waiter = unsafe { &*waiter.cast::<crate::wait_event::Waiter>() };
    if let Some(co) = event_ready(waiter, Events::from_bits(KIND)) {
        wake(co);
    }
    0
}

/// Registers the coroutine socket (`Socket::init_reactor`), poll and event-waiter
/// handlers through `set(fd_type, events, handler)`.
pub fn swr_v1_hook_init_reactor(mut set: impl FnMut(c_int, Events, Handler)) {
    const READ: c_int = Events::READ.bits();
    const WRITE: c_int = Events::WRITE.bits();
    const ERROR: c_int = Events::ERROR.bits();
    let poll = FdType::SW_FD_CO_POLL as c_int;
    let waiter = FdType::SW_FD_CO_EVENT as c_int;
    let socket = FdType::SW_FD_CO_SOCKET as c_int;
    let handlers: [(c_int, Events, Handler); 9] = [
        (
            socket,
            Events::READ,
            socket_members::readable_event_callback,
        ),
        (
            socket,
            Events::WRITE,
            socket_members::writable_event_callback,
        ),
        (socket, Events::ERROR, socket_members::error_event_callback),
        (poll, Events::READ, poll_ready::<READ>),
        (poll, Events::WRITE, poll_ready::<WRITE>),
        (poll, Events::ERROR, poll_ready::<ERROR>),
        (waiter, Events::READ, waiter_ready::<READ>),
        (waiter, Events::WRITE, waiter_ready::<WRITE>),
        (waiter, Events::ERROR, waiter_ready::<ERROR>),
    ];
    for (fd_type, events, handler) in handlers {
        set(fd_type, events, handler);
    }
}

/// `static void System::init_reactor(Reactor *reactor)`: the handlers above, waking
/// coroutines through the thread's deferred queue, then the async-thread pipe's (and
/// `io_uring`'s) completions.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZN6swoole9coroutine6System12init_reactorEPNS_7ReactorE")]
pub extern "C" fn system_init_reactor(reactor: Running<'_>) {
    let mut set = |fd_type, events: Events, handler| {
        swoole_rs_reactor::reactor_upstream::set_handler(reactor, fd_type, events, Some(handler));
    };
    swr_v1_hook_init_reactor(&mut set);
    set(
        FdType::SW_FD_AIO as c_int,
        Events::READ,
        crate::async_threads::callback,
    );
    #[cfg(feature = "iouring")]
    set(FdType::SW_FD_IOURING as c_int, Events::READ, iouring_ready);
}

/// `Iouring::callback`.
#[cfg(all(feature = "upstream-symbols", feature = "iouring"))]
extern "C" fn iouring_ready(_: Running<'_>, _: &mut Event) -> c_int {
    crate::iouring::ready()
}

/// `sw_reactor()->defer([co] { co->resume(); })`, or the unit tests' recorder.
fn wake(co: Parked) {
    #[cfg(test)]
    if let Some(wake) = tests::WAKE {
        return wake(co);
    }
    // Upstream dereferences the thread's null reactor; abort instead.
    if swoole_rs_reactor::event_api::swoole_event_defer_value(co).is_err() {
        swoole_rs_corebase::fatal("no reactor to defer the resume to");
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    clippy::unnecessary_wraps,
    reason = "the fakes have EventWaitOps' signatures and assert what fired while parked"
)]
mod tests {
    use super::*;
    use crate::wait_event::{EventWaitOps, Registered, wait_with};
    use std::cell::{Cell, RefCell};
    use swoole_rs_reactor::SocketRecord;

    const CO: *mut c_void = 0x10 as *mut c_void;

    thread_local! {
        static HANDLERS: RefCell<Vec<(c_int, Events, Handler)>> = const { RefCell::new(Vec::new()) };
        static SOCKET: Cell<*mut SocketRecord> = const { Cell::new(core::ptr::null_mut()) };
        static WOKEN: Cell<u32> = const { Cell::new(0) };
    }

    fn set(fd_type: c_int, events: Events, handler: Handler) {
        HANDLERS.with_borrow_mut(|handlers| handlers.push((fd_type, events, handler)));
    }
    /// Records the wake in place of deferring it.
    pub(super) const WAKE: Option<fn(Parked)> = Some(wake);
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the native wake's signature, which defers it"
    )]
    fn wake(co: Parked) {
        assert_eq!(co.co(), CO);
        WOKEN.set(WOKEN.get() + 1);
    }
    fn handler(fd_type: FdType, events: Events) -> Handler {
        HANDLERS.with_borrow(|handlers| {
            handlers
                .iter()
                .find(|(t, e, _)| *t == fd_type as c_int && *e == events)
                .unwrap()
                .2
        })
    }

    extern "C" fn current() -> *mut c_void {
        CO
    }
    /// The fake reactor's socket record, freed on drop.
    struct Socket(*mut SocketRecord);

    impl Drop for Socket {
        fn drop(&mut self) {
            drop(unsafe { Box::from_raw(self.0) });
        }
    }

    fn add(fd: i32, _: Events, waiter: Registered) -> Result<Socket, i32> {
        let socket = SocketRecord::with_fd(fd, FdType::SW_FD_CO_EVENT as c_int).into_raw();
        unsafe { (*socket).fields.object = waiter.as_ptr() };
        SOCKET.set(socket);
        Ok(Socket(socket))
    }
    /// The reactor fires write, then read, on the waiter's socket.
    fn park(_: f64) -> Result<(), i32> {
        let mut event = unsafe {
            Event::of(
                7,
                0,
                FdType::SW_FD_CO_EVENT as c_int,
                swoole_rs_reactor::Socket::from_ptr(SOCKET.get()),
            )
        };
        for events in [Events::WRITE, Events::READ] {
            let ready = handler(FdType::SW_FD_CO_EVENT, events);
            assert_eq!(
                unsafe { ready(Running::new(core::ptr::null_mut()), &mut event) },
                0
            );
        }
        Ok(())
    }

    #[test]
    fn registers_nine_handlers_and_wakes_a_waiter_once() {
        swr_v1_hook_init_reactor(set);
        assert_eq!(HANDLERS.with_borrow(Vec::len), 9);
        for fd_type in [
            FdType::SW_FD_CO_SOCKET,
            FdType::SW_FD_CO_POLL,
            FdType::SW_FD_CO_EVENT,
        ] {
            for events in [Events::READ, Events::WRITE, Events::ERROR] {
                handler(fd_type, events);
            }
        }
        let ops = EventWaitOps { current, add, park };
        let waited = wait_with(7, Events::READ | Events::WRITE, 1.0, &ops);
        assert_eq!(
            (waited.unwrap_or(Events::NONE).bits(), WOKEN.get()),
            (Events::RDWR.bits(), 1)
        );
    }
}
