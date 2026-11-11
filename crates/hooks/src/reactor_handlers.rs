//! `System::init_reactor`'s readiness handlers for coroutine sockets
//! (`SW_FD_CO_SOCKET`), hooked polls (`SW_FD_CO_POLL`) and event waiters
//! (`SW_FD_CO_EVENT`), registered through `set`; a ready waiter's coroutine resumes
//! through [`wake`], deferred on the thread's reactor.
use crate::socket_members;
use crate::wait_event::{ERROR, Parked, READ, WRITE, event_ready};
use core::ffi::{c_int, c_void};
use swoole_rs_corebase::constants::FdType;
use swoole_rs_reactor::{Event, Handler, Running};

#[cfg(not(test))]
use defer_resume as wake;
#[cfg(test)]
use tests::wake;

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
    crate::poll::ready(unsafe { &*task.cast::<crate::poll::Task<'_>>() }, fd, KIND);
    0
}

extern "C" fn waiter_ready<const KIND: i32>(_: Running<'_>, event: &mut Event) -> c_int {
    let (waiter, _) = fired(event);
    // SAFETY: a `SW_FD_CO_EVENT` socket, whose object is its parked waiter.
    let waiter = unsafe { &*waiter.cast::<crate::wait_event::Waiter>() };
    if let Some(co) = event_ready(waiter, KIND) {
        wake(co);
    }
    0
}

/// Registers the coroutine socket (`Socket::init_reactor`), poll and event-waiter
/// handlers through `set(fd_type, events, handler)`.
pub fn swr_v1_hook_init_reactor(mut set: impl FnMut(c_int, c_int, Handler)) {
    let poll = FdType::SW_FD_CO_POLL as c_int;
    let waiter = FdType::SW_FD_CO_EVENT as c_int;
    let socket = FdType::SW_FD_CO_SOCKET as c_int;
    let handlers: [(c_int, c_int, Handler); 9] = [
        (socket, READ, socket_members::readable_event_callback),
        (socket, WRITE, socket_members::writable_event_callback),
        (socket, ERROR, socket_members::error_event_callback),
        (poll, READ, poll_ready::<READ>),
        (poll, WRITE, poll_ready::<WRITE>),
        (poll, ERROR, poll_ready::<ERROR>),
        (waiter, READ, waiter_ready::<READ>),
        (waiter, WRITE, waiter_ready::<WRITE>),
        (waiter, ERROR, waiter_ready::<ERROR>),
    ];
    for (fd_type, events, handler) in handlers {
        set(fd_type, events, handler);
    }
}

/// `static void System::init_reactor(Reactor *reactor)`: the handlers above, waking
/// coroutines through the thread's deferred queue, then the async-thread pipe's (and
/// `io_uring`'s) completions.
///
/// # Safety
/// `reactor` is the calling thread's live `Reactor`.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_ZN6swoole9coroutine6System12init_reactorEPNS_7ReactorE")]
pub unsafe extern "C" fn system_init_reactor(reactor: *mut c_void) {
    let mut set = |fd_type, events, handler| {
        // SAFETY: the caller's live reactor.
        unsafe {
            swoole_rs_reactor::reactor_upstream::reactor_set_handler(
                reactor,
                fd_type,
                events,
                Some(handler),
            );
        }
    };
    swr_v1_hook_init_reactor(&mut set);
    set(
        FdType::SW_FD_AIO as c_int,
        READ,
        crate::async_threads::callback,
    );
    #[cfg(feature = "iouring")]
    set(FdType::SW_FD_IOURING as c_int, READ, iouring_ready);
}

/// `Iouring::callback`.
#[cfg(all(feature = "upstream-symbols", feature = "iouring"))]
extern "C" fn iouring_ready(_: Running<'_>, _: &mut Event) -> c_int {
    crate::iouring::ready()
}

/// `sw_reactor()->defer([co] { co->resume(); })`.
#[cfg(not(test))]
fn defer_resume(co: Parked) {
    // Upstream dereferences the thread's null reactor; abort instead.
    if swoole_rs_reactor::event_api::swoole_event_defer_value(co).is_err() {
        std::process::abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wait_event::{EventWaitOps, Registered, swr_v1_hook_wait_event};
    use std::cell::{Cell, RefCell};
    use swoole_rs_reactor::SocketRecord;

    const CO: *mut c_void = 0x10 as *mut c_void;

    thread_local! {
        static HANDLERS: RefCell<Vec<(c_int, c_int, Handler)>> = const { RefCell::new(Vec::new()) };
        static SOCKET: Cell<*mut SocketRecord> = const { Cell::new(core::ptr::null_mut()) };
        static WOKEN: Cell<u32> = const { Cell::new(0) };
    }

    fn set(fd_type: c_int, events: c_int, handler: Handler) {
        HANDLERS.with_borrow_mut(|handlers| handlers.push((fd_type, events, handler)));
    }
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the native wake's signature, which defers it"
    )]
    pub(super) fn wake(co: Parked) {
        assert_eq!(co.co(), CO);
        WOKEN.set(WOKEN.get() + 1);
    }
    fn handler(fd_type: FdType, events: c_int) -> Handler {
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
            unsafe { SocketRecord::free(self.0) };
        }
    }

    #[expect(clippy::unnecessary_wraps, reason = "EventWaitOps::add's signature")]
    fn add(fd: i32, _: i32, waiter: Registered) -> Result<Socket, i32> {
        let socket = SocketRecord::with_fd(fd, FdType::SW_FD_CO_EVENT as c_int).into_raw();
        unsafe { (*socket).fields.object = waiter.as_ptr() };
        SOCKET.set(socket);
        Ok(Socket(socket))
    }
    /// The reactor fires write, then read, on the waiter's socket.
    fn park(_: f64) -> i32 {
        let mut event =
            unsafe { Event::new(7, 0, FdType::SW_FD_CO_EVENT as c_int, SOCKET.get().cast()) };
        for events in [WRITE, READ] {
            let ready = handler(FdType::SW_FD_CO_EVENT, events);
            assert_eq!(
                unsafe { ready(Running::new(core::ptr::null_mut()), &mut event) },
                0
            );
        }
        0
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
            for events in [READ, WRITE, ERROR] {
                handler(fd_type, events);
            }
        }
        let ops = EventWaitOps { current, add, park };
        let waited = swr_v1_hook_wait_event(7, READ | WRITE, 1.0, &ops);
        assert_eq!((waited.result, WOKEN.get()), (READ | WRITE, 1));
    }
}
